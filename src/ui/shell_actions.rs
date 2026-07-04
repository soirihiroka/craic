use super::app_menu::launch_workspace_in_new_instance;
use super::dialogs::show_error_dialog;
use super::git_actions::{GitAction, execute_git_action};
use super::preferences::show_preferences_window;
use super::{
    AGENT_SESSION_NOTIFICATION_ACTION, AppState, activate_page, active_workspace_from_config,
    agent_session_notification_id, apply_workspace_color, dispatch_page_command,
    pages::PageCommand, refresh, refresh_active_page, refresh_active_repo_metadata,
};
use crate::config::{ConfiguredWorkspace, WorkspaceProvider};
use crate::git;
use crate::system::SystemProviderRegistry;
use crate::system::providers::local::LocalProvider;
use adw::prelude::*;
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

pub(super) fn connect_shell_actions(state: &Rc<AppState>) {
    connect_app_actions(state);
    connect_open_search_shortcut(state);
    connect_repository_picker(state);
    connect_sidebar_mode_buttons(state);
}

fn connect_app_actions(state: &Rc<AppState>) {
    if let Some(app) = state.window.application() {
        let open_repo =
            gio::SimpleAction::new("open_repo", Some(gtk::glib::VariantTy::new("s").unwrap()));
        open_repo.connect_activate({
            let state = state.clone();
            move |_, parameter| {
                if let Some(val) = parameter.and_then(|v| v.str()) {
                    let workspace = crate::workspace::workspace_from_selection_id(val);
                    prompt_workspace_open(&state, workspace);
                }
            }
        });
        app.add_action(&open_repo);

        let pull_action = gio::SimpleAction::new("pull", None);
        pull_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                execute_git_action(&state, GitAction::Pull);
            }
        });
        app.add_action(&pull_action);
        app.set_accels_for_action("app.pull", &["<Control>p"]);

        let push_action = gio::SimpleAction::new("push", None);
        push_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                execute_git_action(&state, GitAction::Push);
            }
        });
        app.add_action(&push_action);
        app.set_accels_for_action("app.push", &["<Control>u"]);

        let refresh_action = gio::SimpleAction::new("refresh", None);
        refresh_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                refresh(&state, Some("Workspace status refreshed.".to_string()));
            }
        });
        app.add_action(&refresh_action);
        app.set_accels_for_action("app.refresh", &["<Control>r"]);

        let refresh_page_action = gio::SimpleAction::new("refresh_page", None);
        refresh_page_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                refresh_active_page(&state);
            }
        });
        app.add_action(&refresh_page_action);
        app.set_accels_for_action("app.refresh_page", &["F5"]);

        let open_agent_session = gio::SimpleAction::new(
            AGENT_SESSION_NOTIFICATION_ACTION,
            Some(gtk::glib::VariantTy::new("t").unwrap()),
        );
        open_agent_session.connect_activate({
            let state = state.clone();
            move |_, parameter| {
                let Some(session_id) = parameter.and_then(|value| value.get::<u64>()) else {
                    log::warn!("agent notification activated without valid session target");
                    return;
                };

                log::info!("agent notification activated session_id={session_id}");
                state.window.present();
                let result =
                    dispatch_page_command(&state, PageCommand::OpenAgentSession(session_id));
                log::debug!(
                    "agent notification dispatch completed session_id={} handled={}",
                    session_id,
                    result != super::pages::PageCommandResult::Ignored
                );

                if let Some(app) = state.window.application() {
                    app.withdraw_notification(&agent_session_notification_id(session_id));
                }
            }
        });
        app.add_action(&open_agent_session);

        let preferences = gio::SimpleAction::new("preferences", None);
        preferences.connect_activate({
            let state = state.clone();
            move |_, _| {
                let system = state.system_ref.borrow().clone();
                let workspace = state.workspace_ref.borrow().clone();
                show_preferences_window(
                    &state.window,
                    super::git_handle_for_workspace(&state, &system.id, &workspace),
                    state.providers.github(&system.id, &workspace),
                );
            }
        });
        app.add_action(&preferences);
        app.set_accels_for_action("app.preferences", &["<Control>comma"]);
    }
}

fn connect_open_search_shortcut(state: &Rc<AppState>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let state = state.clone();

        move |_, key, _, modifiers| {
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            if !(ctrl && !alt && matches!(key, gdk::Key::f | gdk::Key::F)) {
                return gtk::glib::Propagation::Proceed;
            }

            let index = state.active_page.get();
            let Some(page) = state.pages.get(index) else {
                return gtk::glib::Propagation::Proceed;
            };

            let handled = if state.page_host.left_hovered() {
                page.toggle_left_search()
            } else if state.page_host.right_hovered() {
                page.toggle_right_search()
            } else {
                false
            };

            if handled {
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        }
    });
    state.window.add_controller(keys);
}

fn connect_repository_picker(state: &Rc<AppState>) {
    state.sidebar.repository_picker.connect_item_activated({
        let state = state.clone();
        move |id| {
            let workspace = crate::workspace::workspace_from_selection_id(&id);
            prompt_workspace_open(&state, workspace);
        }
    });

    state.sidebar.repository_picker.connect_add_clicked({
        let state = state.clone();
        move || show_create_workspace_dialog(&state)
    });

    state.sidebar.repository_picker.connect_opened({
        let state = state.clone();
        move || state.sidebar.load_repos_async()
    });
}

fn connect_sidebar_mode_buttons(state: &Rc<AppState>) {
    for (index, button) in state.sidebar.mode_switcher.buttons.iter().enumerate() {
        button.connect_toggled({
            let state = state.clone();
            move |button| {
                if button.is_active() {
                    activate_page(&state, index);
                }
            }
        });
    }
}

fn set_active_workspace(state: &Rc<AppState>, workspace: ConfiguredWorkspace) {
    let active = active_workspace_from_config(&state.providers, &workspace);
    let item_id = workspace.selection_id();
    *state.repo_path.borrow_mut() = active.repo_path.clone();
    state.system_ref.replace(active.system_ref);
    state.workspace_ref.replace(active.workspace_ref);
    for page in &state.pages {
        page.workspace_changed();
    }
    apply_workspace_color(state);
    crate::config::save_last_workspace(&workspace);
    refresh_active_repo_metadata(state, Some(item_id));
}

fn prompt_workspace_open(state: &Rc<AppState>, workspace: ConfiguredWorkspace) {
    let label = workspace.label();
    let dialog = adw::AlertDialog::builder()
        .heading("Open Workspace")
        .body(format!("Open {label} in this window or a new window?"))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("current", "Open Here");
    dialog.add_response("new", "Open in New Window");
    dialog.set_default_response(Some("current"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("current", adw::ResponseAppearance::Suggested);

    dialog.choose(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        move |response| match response.as_str() {
            "current" => {
                log::info!("workspace open selected target=current label={label}");
                open_workspace_here(&state, workspace.clone(), None);
            }
            "new" => {
                log::info!("workspace open selected target=new-window label={label}");
                match open_workspace_in_new_window(&workspace) {
                    Ok(()) => {}
                    Err(err) => show_error_dialog(&state.window, "Open Workspace Failed", &err),
                }
            }
            _ => {}
        }
    });
}

fn open_workspace_here(
    state: &Rc<AppState>,
    workspace: ConfiguredWorkspace,
    message: Option<String>,
) {
    set_active_workspace(state, workspace);
    refresh(state, message);
}

fn open_workspace_in_new_window(workspace: &ConfiguredWorkspace) -> Result<(), String> {
    let path = local_workspace_launch_path(workspace)?;
    launch_workspace_in_new_instance(&path)
}

fn local_workspace_launch_path(workspace: &ConfiguredWorkspace) -> Result<PathBuf, String> {
    match &workspace.provider {
        WorkspaceProvider::Local => {}
        WorkspaceProvider::Ssh { .. } => {
            return Err(
                "Opening remote workspaces in a new window is not supported yet.".to_string(),
            );
        }
    }

    let path = crate::config::expand_config_path_for_ui(&workspace.path)
        .unwrap_or_else(|| PathBuf::from(&workspace.path));
    if !path.exists() {
        return Err(format!("Workspace path does not exist: {}", path.display()));
    }
    Ok(path.canonicalize().unwrap_or(path))
}

fn show_create_workspace_dialog(state: &Rc<AppState>) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Create Workspace")
        .content_width(560)
        .build();

    let workspace_roots = Rc::new(
        crate::config::load()
            .workspace_roots
            .into_iter()
            .filter_map(|workspace_root| {
                workspace_root
                    .provider
                    .is_local()
                    .then(|| {
                        crate::config::expand_config_path_for_ui(&workspace_root.path)
                            .map(|path| (workspace_root, path.canonicalize().unwrap_or(path)))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>(),
    );
    let workspace_root_labels = workspace_roots
        .iter()
        .map(|(_, path)| path.display().to_string())
        .collect::<Vec<_>>();
    let workspace_root_label_refs = workspace_root_labels
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let workspace_root_model = gtk::StringList::new(&workspace_root_label_refs);
    let root_path = Rc::new(RefCell::new(
        workspace_roots.first().map(|(_, path)| path.to_path_buf()),
    ));
    let auto_name = Rc::new(Cell::new(true));
    let updating_name = Rc::new(Cell::new(false));
    let create_loading = Rc::new(Cell::new(false));

    let root_row = adw::ComboRow::builder().title("Workspace Root").build();
    root_row.set_model(Some(&workspace_root_model));
    root_row.set_sensitive(!workspace_roots.is_empty());
    if !workspace_roots.is_empty() {
        root_row.set_selected(0);
        if let Some((_, path)) = workspace_roots.first() {
            root_row.set_subtitle(&path.display().to_string());
        }
    }

    let name_row = adw::EntryRow::builder().title("Repository Name").build();
    let remote_row = adw::EntryRow::builder().title("Remote Git Source").build();
    remote_row.set_tooltip_text(Some("Optional git remote URL to clone."));

    let cancel_button = gtk::Button::builder().label("Cancel").build();
    let create_spinner = adw::Spinner::new();
    create_spinner.set_size_request(16, 16);
    create_spinner.set_visible(false);
    let create_label = gtk::Label::new(Some("Create"));
    let create_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    create_content.append(&create_spinner);
    create_content.append(&create_label);
    let create_button = gtk::Button::builder()
        .child(&create_content)
        .sensitive(false)
        .build();
    create_button.add_css_class("suggested-action");

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    actions.append(&cancel_button);
    actions.append(&create_button);

    let workspace_group = adw::PreferencesGroup::new();
    workspace_group.add(&root_row);
    workspace_group.add(&name_row);
    workspace_group.add(&remote_row);

    let action_group = adw::PreferencesGroup::new();
    action_group.add(&actions);

    let page = adw::PreferencesPage::new();
    page.set_title("Create Workspace");
    page.set_icon_name(Some("folder-new-symbolic"));
    page.add(&workspace_group);
    page.add(&action_group);
    dialog.add(&page);

    let update_create_state: Rc<dyn Fn()> = Rc::new({
        let root_path = root_path.clone();
        let name_row = name_row.clone();
        let remote_row = remote_row.clone();
        let create_button = create_button.clone();
        let create_loading = create_loading.clone();

        move || {
            if create_loading.get() {
                create_button.set_sensitive(false);
                return;
            }
            let remote = remote_row.text();
            let remote = remote.trim();
            let name = name_row.text();
            let has_name = !name.trim().is_empty()
                || (!remote.is_empty() && workspace_name_from_remote(remote).is_some());
            let has_root = root_path.borrow().is_some();
            let ready = has_root && has_name;
            create_button.set_sensitive(ready);
        }
    });
    update_create_state();

    root_row.connect_selected_notify({
        let workspace_roots = workspace_roots.clone();
        let root_path = root_path.clone();
        let update_create_state = update_create_state.clone();
        let root_row = root_row.clone();

        move |row| {
            if workspace_roots.is_empty() {
                root_path.replace(None);
                root_row.set_subtitle("No workspace roots available.");
                update_create_state();
                return;
            }
            let selected = row.selected() as usize;
            if let Some((_, path)) = workspace_roots.get(selected) {
                root_row.set_subtitle(&path.display().to_string());
                root_path.replace(Some(path.clone()));
            } else {
                root_row.set_subtitle("Invalid workspace root.");
                root_path.replace(None);
            }
            update_create_state();
        }
    });

    name_row.connect_changed({
        let auto_name = auto_name.clone();
        let updating_name = updating_name.clone();
        let update_create_state = update_create_state.clone();

        move |row| {
            if !updating_name.get() {
                auto_name.set(row.text().trim().is_empty());
            }
            update_create_state();
        }
    });

    remote_row.connect_changed({
        let name_row = name_row.clone();
        let auto_name = auto_name.clone();
        let updating_name = updating_name.clone();
        let update_create_state = update_create_state.clone();

        move |row| {
            if auto_name.get() || name_row.text().trim().is_empty() {
                let next_name = workspace_name_from_remote(&row.text()).unwrap_or_default();
                updating_name.set(true);
                name_row.set_text(&next_name);
                updating_name.set(false);
                auto_name.set(true);
            }
            update_create_state();
        }
    });

    cancel_button.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
        }
    });

    create_button.connect_clicked({
        let state = state.clone();
        let dialog = dialog.clone();
        let root_path = root_path.clone();
        let name_row = name_row.clone();
        let remote_row = remote_row.clone();
        let create_button = create_button.clone();
        let create_spinner = create_spinner.clone();
        let create_label = create_label.clone();
        let update_create_state = update_create_state.clone();
        let create_loading = create_loading.clone();

        move |_| {
            let request = match create_workspace_request(
                root_path.borrow().clone(),
                &name_row.text(),
                &remote_row.text(),
            ) {
                Ok(request) => request,
                Err(err) => {
                    show_error_dialog(&state.window, "Create Workspace Failed", &err);
                    return;
                }
            };

            log::info!(
                "workspace creation requested root={} name={} remote_present={}",
                request.root.display(),
                request.name,
                request.remote.is_some()
            );
            create_loading.set(true);
            set_create_button_loading(&create_button, &create_spinner, &create_label, true);

            let (sender, receiver) = mpsc::channel();
            let providers = state.providers.clone();
            thread::spawn(move || {
                let result = create_workspace(&providers, request);
                let _ = sender.send(result);
            });

            gtk::glib::timeout_add_local(Duration::from_millis(100), {
                let state = state.clone();
                let dialog = dialog.clone();
                let create_button = create_button.clone();
                let create_spinner = create_spinner.clone();
                let create_label = create_label.clone();
                let update_create_state = update_create_state.clone();
                let create_loading = create_loading.clone();

                move || match receiver.try_recv() {
                    Ok(Ok((path, message))) => {
                        dialog.close();
                        open_workspace_here(
                            &state,
                            ConfiguredWorkspace::local(path.to_string_lossy().to_string()),
                            Some(message),
                        );
                        state.sidebar.load_repos_async();
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(err)) => {
                        create_loading.set(false);
                        set_create_button_loading(
                            &create_button,
                            &create_spinner,
                            &create_label,
                            false,
                        );
                        update_create_state();
                        show_error_dialog(&state.window, "Create Workspace Failed", &err);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(TryRecvError::Disconnected) => {
                        create_loading.set(false);
                        set_create_button_loading(
                            &create_button,
                            &create_spinner,
                            &create_label,
                            false,
                        );
                        update_create_state();
                        let err = "Workspace creation stopped unexpectedly.";
                        show_error_dialog(&state.window, "Create Workspace Failed", err);
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        }
    });

    dialog.present(Some(&state.window));
}

#[derive(Debug)]
struct CreateWorkspaceRequest {
    root: PathBuf,
    name: String,
    remote: Option<String>,
}

fn create_workspace_request(
    root: Option<PathBuf>,
    name: &str,
    remote: &str,
) -> Result<CreateWorkspaceRequest, String> {
    let root = root.ok_or_else(|| "Choose a workspace root.".to_string())?;
    if !root.is_dir() {
        return Err(format!(
            "Workspace root is not a directory: {}",
            root.display()
        ));
    }
    let remote = text_option(remote);
    let name = if name.trim().is_empty() {
        remote
            .as_deref()
            .and_then(workspace_name_from_remote)
            .unwrap_or_default()
    } else {
        name.trim().to_string()
    };
    let name = validated_workspace_name(&name)?;

    Ok(CreateWorkspaceRequest { root, name, remote })
}

fn create_workspace(
    providers: &SystemProviderRegistry,
    request: CreateWorkspaceRequest,
) -> Result<(PathBuf, String), String> {
    let destination = request.root.join(&request.name);
    ensure_destination_available(&destination)?;

    if let Some(remote) = request.remote {
        let workspace = LocalProvider::workspace_for_path(&request.root);
        let shell = providers
            .shell(&LocalProvider::new().system_ref().id, &workspace)
            .ok_or_else(|| "Local shell access is unavailable.".to_string())?;
        let message = git::clone_repository_with_shell(
            shell,
            workspace.root.clone(),
            &remote,
            &request.name,
        )?;
        return Ok((destination, message));
    }

    log::info!(
        "workspace folder create start path={}",
        destination.display()
    );
    if !destination.exists() {
        std::fs::create_dir_all(&destination).map_err(|err| {
            format!(
                "Could not create workspace folder {}: {err}",
                destination.display()
            )
        })?;
    }
    log::info!(
        "workspace folder create complete path={}",
        destination.display()
    );
    Ok((destination, "Workspace created.".to_string()))
}

fn ensure_destination_available(destination: &Path) -> Result<(), String> {
    if !destination.exists() {
        return Ok(());
    }
    if !destination.is_dir() {
        return Err(format!(
            "Destination already exists and is not a folder: {}",
            destination.display()
        ));
    }
    let mut entries = std::fs::read_dir(destination).map_err(|err| {
        format!(
            "Could not inspect destination folder {}: {err}",
            destination.display()
        )
    })?;
    if entries.next().is_some() {
        return Err(format!(
            "Destination folder is not empty: {}",
            destination.display()
        ));
    }
    Ok(())
}

fn text_option(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn workspace_name_from_remote(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/');
    if remote.is_empty() {
        return None;
    }
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let name = remote
        .rsplit(|ch| ch == '/' || ch == ':')
        .next()
        .unwrap_or(remote);
    validated_workspace_name(name).ok()
}

fn validated_workspace_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Repository name is required.".to_string());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("Repository name must be a single folder name.".to_string());
    }
    Ok(name.to_string())
}

fn set_create_button_loading(
    button: &gtk::Button,
    spinner: &adw::Spinner,
    label: &gtk::Label,
    loading: bool,
) {
    spinner.set_visible(loading);
    label.set_label(if loading { "Creating..." } else { "Create" });
    button.set_sensitive(!loading);
}
