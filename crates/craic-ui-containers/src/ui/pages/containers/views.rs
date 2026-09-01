impl ContainersPage {
    pub fn new(ctx: PageContext) -> Self {
        let left = LeftPane::new();
        let right = Rc::new(RightPane::new());
        let inventory = Rc::new(RefCell::new(None));
        let selected = Rc::new(RefCell::new(None));
        let expanded_groups = Rc::new(RefCell::new(HashSet::new()));
        let refresh_generation = Rc::new(Cell::new(0));
        let inspect_generation = Rc::new(Cell::new(0));
        let active_context_menu = Rc::new(RefCell::new(None));

        right.show_empty("Select a container or Compose project.");

        let page = Self {
            ctx,
            left,
            right,
            inventory,
            selected,
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        };
        page.connect_search();
        page
    }

    fn refresh_containers(&self) {
        self.refresh_containers_with_completion(None);
    }

    fn refresh_containers_with_completion(&self, completion: Option<PageRefreshComplete>) {
        refresh_containers(RefreshRequest {
            ctx: self.ctx.clone(),
            tree: self.left.tree.clone(),
            state: self.left.state.clone(),
            right: self.right.clone(),
            inventory: self.inventory.clone(),
            selected: self.selected.clone(),
            expanded_groups: self.expanded_groups.clone(),
            refresh_generation: self.refresh_generation.clone(),
            inspect_generation: self.inspect_generation.clone(),
            active_context_menu: self.active_context_menu.clone(),
            completion,
        });
    }

    fn connect_search(&self) {
        self.left
            .search_panel
            .set_key_capture_widget(&self.left.root);
        self.left.search_panel.install_shortcuts(&self.left.root);
        self.left
            .search_panel
            .install_shortcuts(&self.left.tree.root);
        self.left
            .search_panel
            .install_shortcuts(&self.left.state.stack);
        self.left.search_panel.install_shortcuts(&self.right.root);
        self.left.search_panel.connect_query_changed({
            let ctx = self.ctx.clone();
            let tree = self.left.tree.clone();
            let state = self.left.state.clone();
            let right = self.right.clone();
            let inventory = self.inventory.clone();
            let selected = self.selected.clone();
            let expanded_groups = self.expanded_groups.clone();
            let refresh_generation = self.refresh_generation.clone();
            let inspect_generation = self.inspect_generation.clone();
            let active_context_menu = self.active_context_menu.clone();

            move |query| {
                let query = query.trim().to_string();
                state.search_query.replace(query.clone());
                log::debug!("containers search updated query_len={}", query.len());
                render_container_tree(
                    &ctx,
                    &tree,
                    &state,
                    &right,
                    &inventory,
                    &selected,
                    &expanded_groups,
                    &refresh_generation,
                    &inspect_generation,
                    &active_context_menu,
                );
            }
        });
        self.left.search_panel.connect_closed({
            let state = self.left.state.clone();

            move || {
                state.search_query.borrow_mut().clear();
            }
        });
    }
}

impl Page for ContainersPage {
    fn label(&self) -> &'static str {
        "Containers"
    }

    fn icon_name(&self) -> &'static str {
        "container-symbolic"
    }

    fn initialize(&self, completion: PageInitializeComplete) {
        completion(
            self.left.root.clone().upcast(),
            self.right.root.clone().upcast(),
        );
    }

    fn activate(&self) {
        self.refresh_containers();
    }

    fn refresh(&self, _snapshot: &WorkspaceSnapshot, completion: PageRefreshComplete) {
        completion();
    }

    fn refresh_page(&self, completion: PageRefreshComplete) -> PageRefreshRequest {
        log::info!("containers page refresh requested");
        self.refresh_containers_with_completion(Some(completion));
        PageRefreshRequest::Custom
    }

    fn set_error(&self, message: &str) {
        self.left
            .state
            .show_empty("dialog-warning-symbolic", message);
        self.right.show_error("Repository Error", message);
    }

    fn toggle_left_search(&self) -> bool {
        self.left.search_panel.toggle();
        true
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        match command {
            _ => PageCommandResult::Ignored,
        }
    }
}

impl LeftPane {
    fn new() -> Self {
        let loading_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();
        let spinner = adw::Spinner::new();
        spinner.set_size_request(24, 24);
        let loading_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .margin_start(16)
            .margin_end(16)
            .build();
        loading_box.append(&spinner);
        loading_box.append(&loading_label);
        let loading_clamp = adw::Clamp::builder()
            .maximum_size(280)
            .tightening_threshold(220)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .child(&loading_box)
            .build();

        let status_page = adw::StatusPage::builder()
            .icon_name("container-symbolic")
            .title("Press F5 to refresh Containers.")
            .hexpand(true)
            .vexpand(true)
            .build();

        let tree = ContainerTreeView::builder()
            .autoscroll_context("containers")
            .build();
        let search_panel = SearchPanel::new("Search containers");
        search_panel.set_options_visible(false);
        search_panel.set_navigation_visible(false);
        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.add_named(&tree.root, Some("tree"));
        stack.add_named(&loading_clamp, Some("loading"));
        stack.add_named(&status_page, Some("status"));
        stack.set_visible_child_name("loading");
        let state = LeftPaneState {
            stack,
            spinner,
            loading_label,
            status_page,
            search_query: Rc::new(RefCell::new(String::new())),
        };
        state.show_loading("Refreshing containers...");

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .focusable(true)
            .vexpand(true)
            .build();
        root.append(&search_panel.widget());
        root.append(&state.stack);

        Self {
            root,
            search_panel,
            tree,
            state,
        }
    }
}

impl LeftPaneState {
    fn show_tree(&self) {
        self.spinner.set_visible(false);
        self.stack.set_visible_child_name("tree");
    }

    fn show_loading(&self, message: &str) {
        self.spinner.set_visible(true);
        self.loading_label.set_label(message);
        self.stack.set_visible_child_name("loading");
    }

    fn show_empty(&self, icon_name: &str, message: &str) {
        self.spinner.set_visible(false);
        self.status_page.set_icon_name(Some(icon_name));
        self.status_page.set_title(message);
        self.status_page.set_description(None);
        self.stack.set_visible_child_name("status");
    }
}

impl RightPane {
    fn new() -> Self {
        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading", "bold"])
            .build();
        let subtitle_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .build();
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();
        header.append(&title_label);
        header.append(&subtitle_label);

        let status_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();
        let status_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        status_box.append(&status_label);

        let details_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(16)
            .margin_end(16)
            .hexpand(true)
            .build();
        let details_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&details_box)
            .build();

        let inspect_view = code_editor::CodeEditor::new("json", "");
        inspect_view.set_read_only(true);
        inspect_view.root.set_vexpand(true);
        inspect_view.root.set_hexpand(true);

        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.add_named(&status_box, Some("status"));
        stack.add_named(&details_scroller, Some("details"));
        stack.add_named(&inspect_view.root, Some("inspect"));
        stack.set_visible_child_name("status");

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&stack);

        Self {
            root,
            title_label,
            subtitle_label,
            stack,
            status_label,
            details_box,
            inspect_view,
        }
    }

    fn show_empty(&self, message: &str) {
        self.title_label.set_text("Containers");
        self.subtitle_label.set_text("");
        self.status_label.set_text(message);
        self.stack.set_visible_child_name("status");
        clear_box(&self.details_box);
    }

    fn show_error(&self, heading: &str, message: &str) {
        self.title_label.set_text(heading);
        self.subtitle_label.set_text("");
        self.status_label.set_text(message);
        self.stack.set_visible_child_name("status");
        clear_box(&self.details_box);
    }

    fn show_loading(&self, message: &str) {
        self.title_label.set_text("Containers");
        self.subtitle_label.set_text("");
        self.status_label.set_text(message);
        self.stack.set_visible_child_name("status");
    }

    fn show_container<F>(&self, container: &docker::DockerContainer, inspect_action: F)
    where
        F: Fn() + 'static,
    {
        self.title_label.set_text(container.display_name());
        self.subtitle_label.set_text(&container.image);
        clear_box(&self.details_box);

        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::Start)
            .build();
        let inspect_button = gtk::Button::with_label("Inspect");
        inspect_button.connect_clicked(move |_| inspect_action());
        actions.append(&inspect_button);
        self.details_box.append(&actions);

        append_section(
            &self.details_box,
            "Overview",
            vec![
                ("Name", container.name.clone()),
                ("ID", container.short_id().to_string()),
                ("Image", container.image.clone()),
                ("State", container.state.clone()),
                ("Status", container.status.clone()),
                ("Created", container.created_at.clone()),
                ("Running For", container.running_for.clone()),
                ("Ports", display_scalar(&container.ports)),
            ],
        );
        append_section(
            &self.details_box,
            "Networks And Mounts",
            vec![
                ("Networks", display_values(&container.networks)),
                ("Mounts", display_values(&container.mounts)),
            ],
        );
        append_map_section(&self.details_box, "Labels", &container.labels);

        self.stack.set_visible_child_name("details");
    }

    fn show_group(&self, group: &docker::ContainerGroup) {
        self.title_label.set_text(&group.title);
        self.subtitle_label
            .set_text(&format!("{} containers", group.containers.len()));
        clear_box(&self.details_box);

        let running = group
            .containers
            .iter()
            .filter(|container| docker::state_is_running(&container.state))
            .count();
        let stopped = group.containers.len().saturating_sub(running);
        let mut overview = vec![
            ("Containers", group.containers.len().to_string()),
            ("Running", running.to_string()),
            ("Stopped", stopped.to_string()),
        ];
        if let Some(compose) = group.compose_metadata() {
            overview.push(("Project", compose.project.clone()));
            overview.push((
                "Working Directory",
                compose
                    .working_dir
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string()),
            ));
            overview.push(("Compose Files", display_values(&compose.config_files)));
            overview.push((
                "Environment File",
                compose
                    .environment_file
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
            ));
        }
        append_section(&self.details_box, "Overview", overview);

        append_section(
            &self.details_box,
            "Services",
            group
                .containers
                .iter()
                .map(|container| {
                    (
                        container
                            .service
                            .as_deref()
                            .unwrap_or_else(|| container.display_name()),
                        format!("{} · {}", container.display_name(), container.status),
                    )
                })
                .collect(),
        );

        let ports = unique_scalar_values(group.containers.iter().map(|container| &container.ports));
        let networks = unique_values(
            group
                .containers
                .iter()
                .flat_map(|container| &container.networks),
        );
        append_section(
            &self.details_box,
            "Aggregate",
            vec![
                ("Ports", display_values(&ports)),
                ("Networks", display_values(&networks)),
            ],
        );

        self.stack.set_visible_child_name("details");
    }

    fn show_inspect_loading(&self, container: &docker::DockerContainer) {
        self.title_label
            .set_text(&format!("Inspect {}", container.display_name()));
        self.subtitle_label.set_text(&container.image);
        self.inspect_view.set_text("Loading inspect payload...");
        self.inspect_view.set_language("json");
        self.stack.set_visible_child_name("inspect");
    }

    fn show_inspect(&self, container: &docker::DockerContainer, payload: &str) {
        self.title_label
            .set_text(&format!("Inspect {}", container.display_name()));
        self.subtitle_label.set_text(&container.image);
        self.inspect_view.set_document("json", payload);
        self.stack.set_visible_child_name("inspect");
    }
}
