pub fn build_ui(
    app: &adw::Application,
    launch_start: Instant,
    startup_workspace: Option<crate::config::ConfiguredWorkspace>,
    startup_open_location: Option<StartupOpenLocation>,
    startup_error: Option<String>,
) {
    let mut startup = StartupTimer::new(launch_start);
    startup.mark("activate");

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    startup.mark("style-manager");

    register_bundled_fonts();
    startup.mark("register-bundled-fonts");

    install_actions(app);
    startup.mark("install-actions");

    let provider = gtk::CssProvider::new();
    let workspace_color_provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        "{}{}",
        components::search::SEARCH_PANEL_CSS,
        r#"
        .changes-badge, .agent-badge {
            background-color: @accent_bg_color;
            color: @accent_fg_color;
            border-radius: 9999px;
            font-weight: bold;
            font-size: 0.68em;
            min-width: 14px;
            min-height: 14px;
            padding: 0;
        }
        .git-action-card {
            border: 1px solid rgba(53, 132, 228, 0.4);
            background-color: rgba(53, 132, 228, 0.05);
            border-radius: 12px;
        }
        textview.agent-message-text,
        textview.agent-message-text text,
        textview.agent-transcript-text,
        textview.agent-transcript-text text {
            background-color: transparent;
        }
        .agent-transcript-icon {
            -gtk-icon-size: 16px;
            padding-right: 4px;
        }
        .pdf-preview-scroller {
            background-color: @window_bg_color;
        }
        .pdf-preview-page {
            background-color: @view_bg_color;
            border: 1px solid alpha(@view_fg_color, 0.08);
            border-radius: 2px;
            box-shadow: 0 3px 12px rgba(0, 0, 0, 0.32);
        }
        button.terminal-session-close-button {
            min-width: 0;
            min-height: 0;
            padding: 3px;
        }
        .svg-preview-scroller {
            border: none;
            background-color: transparent;
        }
        .markdown-preview {
            border: none;
            background-color: transparent;
            box-shadow: none;
        }
        textview.agent-composer-input,
        textview.agent-composer-input text {
            border-radius: 8px;
            font-size: 1.1em;
        }
        .code-editor-completion-list {
            padding: 4px;
            background-color: transparent;
        }
        .code-editor-completion-row {
            padding: 5px 10px;
        }
        .code-editor-completion-label {
            font-family: monospace;
        }
    "#
    ));
    startup.mark("load-css");

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &workspace_color_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        let icon_theme = gtk::IconTheme::for_display(&display);
        let mut search_paths: Vec<PathBuf> = asset_search_paths()
            .into_iter()
            .filter(|path| path.is_dir())
            .collect();

        for path in icon_theme.search_path() {
            if !search_paths.contains(&path) {
                search_paths.push(path);
            }
        }
        let search_path_refs: Vec<&std::path::Path> =
            search_paths.iter().map(|path| path.as_path()).collect();
        icon_theme.set_search_path(&search_path_refs);
        log::info!(
            "startup icon search paths configured count={}",
            search_path_refs.len()
        );
    } else {
        log::warn!("startup icon search paths skipped because GTK display is unavailable");
    }
    startup.mark("configure-display-assets");

    let menu = app_menu();
    startup.mark("build-app-menu");

    let provider_registry = SystemProviderRegistry::new();
    startup.mark("create-provider-registry");

    let active_workspace = initial_workspace(&provider_registry, startup_workspace.as_ref());
    startup.mark("resolve-initial-workspace");

    let repo_path = active_workspace.repo_path.clone();
    let initial_workspace_key = active_workspace.workspace_ref.id.to_string();
    let initial_snapshot: Option<git::WorkspaceSnapshot> = None;

    let repo_path_cell = Rc::new(RefCell::new(repo_path.clone()));
    let system_ref_cell = Rc::new(RefCell::new(active_workspace.system_ref));
    let workspace_ref_cell = Rc::new(RefCell::new(active_workspace.workspace_ref));
    let window_cell = Rc::new(RefCell::new(None::<adw::ApplicationWindow>));
    let git_action_running = Rc::new(Cell::new(false));
    let state_slot: Rc<RefCell<Weak<AppState>>> = Rc::new(RefCell::new(Weak::new()));

    let page_context = pages::PageContext::new(
        repo_path_cell.clone(),
        system_ref_cell.clone(),
        workspace_ref_cell.clone(),
        provider_registry.clone(),
        window_cell.clone(),
        git_action_running.clone(),
        Rc::new({
            let state_slot = state_slot.clone();
            move |message, show_toast| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    refresh_with_toast(&state, message, show_toast);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move || {
                if let Some(state) = state_slot.borrow().upgrade() {
                    run_git_action(&state);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |message| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    state.show_toast(message);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |working_dir| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    let system = state.system_ref.borrow().clone();
                    let workspace = state.workspace_ref.borrow().clone();
                    let Some(shell) = state.providers.shell(&system.id, &workspace) else {
                        return Err("Terminal is unavailable for this workspace.".to_string());
                    };
                    let command = shell.interactive_shell(Some(working_dir))?;
                    let title = shell.command_display(&command);
                    state.content.run_shell_command(&command, &title)
                } else {
                    Err("Application is not ready.".to_string())
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |path, line, column| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    prompt_open_external_terminal_path(&state, path, line, column);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |command, title| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    state.content.run_shell_command(command, title)
                } else {
                    Err("Application is not ready.".to_string())
                }
            }
        }),
        Rc::new(|workspace_key, git_handle, on_result| {
            request_provider_git_snapshot(workspace_key, git_handle, on_result);
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move || {
                if let Some(state) = state_slot.borrow().upgrade() {
                    for page in &state.pages {
                        let badge = page.badge().map(|badge| Badge {
                            text: badge.text().to_string(),
                            attention: false,
                        });
                        if let Err(command) = state.app_handle.try_send(AppCommand::SetPageBadge {
                            page: page.id(),
                            badge,
                        }) {
                            log::warn!("GTK page badge queue rejected command={command:?}");
                        }
                    }
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |command| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    match command {
                        PageCommand::OpenFileLocation { path, line, column } => {
                            route_open_file_location(&state, path, line, column);
                        }
                        command => {
                            dispatch_page_command(&state, command);
                        }
                    }
                }
            }
        }),
    );
    startup.mark("build-page-context");

    let pages = pages::build_pages(page_context);
    startup.mark("build-pages");

    let sidebar = sidebar::build(
        &menu,
        initial_snapshot
            .as_ref()
            .and_then(git::WorkspaceSnapshot::repository),
        &initial_workspace_key,
        &workspace_ref_cell.borrow().display_name,
        &system_ref_cell.borrow(),
        &pages,
    );
    startup.mark("build-sidebar");

    let content = content::build(
        &menu,
        initial_snapshot
            .as_ref()
            .and_then(git::WorkspaceSnapshot::repository),
    );
    startup.mark("build-content");

    let page_host = pages::PageHost::new(&sidebar.page_slot(), &content.page_slot());
    let (app_runtime, app_channels) = ApplicationRuntime::start(RuntimeConfig {
        thread_name: "craic-gtk-app".to_string(),
        ..RuntimeConfig::default()
    })
    .expect("failed to start GTK application state runtime");
    let app_handle = app_channels.handle;
    let mut app_events = app_channels.events;
    startup.mark("build-page-host");

    let main_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    main_paned.set_start_child(Some(&sidebar.root));
    main_paned.set_end_child(Some(&content.root));
    main_paned.set_resize_start_child(false);
    main_paned.set_shrink_start_child(false);
    main_paned.set_resize_end_child(true);
    main_paned.set_shrink_end_child(false);
    main_paned.set_position(400);

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&main_paned));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Craic")
        .default_width(1440)
        .default_height(920)
        .content(&toast_overlay)
        .build();
    *window_cell.borrow_mut() = Some(window.clone());
    startup.mark("build-window");

    let state = Rc::new(AppState {
        repo_path: repo_path_cell,
        system_ref: system_ref_cell,
        workspace_ref: workspace_ref_cell,
        providers: provider_registry,
        window: window.clone(),
        toast_overlay,
        sidebar,
        content,
        pages,
        page_host,
        active_page: RefCell::new(None),
        app_runtime: RefCell::new(Some(app_runtime)),
        app_handle,
        page_state_revisions: RefCell::new(HashMap::new()),
        page_service_requests: RefCell::new(HashMap::new()),
        git_action_running,
        last_error: RefCell::new(None),
        last_snapshot: RefCell::new(initial_snapshot.clone()),
        last_snapshot_repo: RefCell::new(initial_snapshot.as_ref().map(|_| repo_path.clone())),
        workspace_generation: Cell::new(Generation::INITIAL),
        workspace_refresh_request: RefCell::new(None),
        repository_monitor: RepositoryMonitor::default(),
        workspace_color_provider,
    });
    *state_slot.borrow_mut() = Rc::downgrade(&state);
    startup.mark("build-app-state");

    if let Err(command) =
        state
            .app_handle
            .try_send(AppCommand::SelectWorkspace(WorkspaceSelection {
                id: WorkspaceId::new(initial_workspace_key.clone()),
            }))
    {
        log::warn!("initial GTK app-core workspace selection rejected command={command:?}");
    }

    apply_workspace_color(&state);
    startup.mark("apply-workspace-color");

    if let Some(location) = startup_open_location {
        let result =
            route_open_file_location(&state, location.path, location.line, location.column);
        if result == PageCommandResult::Ignored {
            log::warn!("startup file location was not handled");
            activate_page(&state, 0);
        }
    } else {
        activate_page(&state, 0);
    }
    startup.mark("activate-initial-page");

    connect_git_actions(&state);
    startup.mark("connect-actions");

    connect_window_close_confirmation(&state);
    startup.mark("connect-close-confirmation");

    window.present();
    startup.mark("present-window");

    let state_weak = Rc::downgrade(&state);
    gtk::glib::spawn_future_local(async move {
        while let Some(event) = app_events.recv().await {
            let Some(state) = state_weak.upgrade() else {
                break;
            };
            apply_app_core_event(&state, event);
        }
    });

    let state_weak = Rc::downgrade(&state);
    window.add_tick_callback(move |_, _| {
        let state_weak = state_weak.clone();
        gtk::glib::idle_add_local_once(move || {
            let Some(state) = state_weak.upgrade() else {
                log::debug!("startup deferred work skipped because application state was dropped");
                return;
            };

            let started = Instant::now();
            refresh_active_repo_metadata(&state, None);
            start_repository_monitor(&state);
            refresh(&state, None);
            log::info!(
                "startup deferred work queued elapsed_ms={}",
                started.elapsed().as_millis()
            );
        });
        gtk::glib::ControlFlow::Break
    });
    startup.mark("queue-post-present-work");

    if let Some(notice) = crate::crash_log::take_latest_crash_notice() {
        log::warn!(
            "showing previous crash notice path={}",
            notice.path.display()
        );
        show_startup_crash_dialog(&window, &notice);
    }
    if let Some(error) = startup_error.as_deref() {
        show_error_dialog(&window, "Open Workspace Failed", error);
    }
}
