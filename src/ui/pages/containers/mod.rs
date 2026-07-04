mod docker;

use super::{
    Page, PageCommand, PageCommandResult, PageContext, PageInitializeComplete, PageRefreshComplete,
    PageRefreshRequest,
};
use crate::git::RepositorySnapshot;
use crate::system::WorkspacePath;
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::ui::components::context_menu::{self, ActionMenuItem, ActionMenuSection};
use crate::ui::components::search::SearchPanel;
use crate::ui::components::tree_view::{self, IconRow, TreeRenderState, TreeRenderer, TreeRow};
use crate::ui::content::code_editor;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const DOCKER_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(75);

pub(super) struct ContainersPage {
    ctx: PageContext,
    left: LeftPane,
    right: Rc<RightPane>,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    Group(String),
    Container(String),
}

type Inventory = docker::ContainerInventory;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerMenuAction {
    ViewLogs,
    AttachShell,
    Inspect,
    Start,
    Stop,
    Restart,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposeMenuAction {
    Logs,
    Start,
    Stop,
    Restart,
    Down,
}

struct LeftPane {
    root: gtk::Box,
    search_panel: SearchPanel,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
}

type ContainerTreeView = tree_view::TreeView<ContainerRowKey, ContainerRowState>;
const CONTAINER_GROUP_COUNT_CLASS: &str = "craic-container-group-count";
const CONTAINER_STATE_CLASS: &str = "craic-container-state";

#[derive(Clone)]
struct LeftPaneState {
    stack: gtk::Stack,
    spinner: adw::Spinner,
    loading_label: gtk::Label,
    status_page: adw::StatusPage,
    search_query: Rc<RefCell<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ContainerRowKey {
    Group(String),
    Container(String),
}

#[derive(Clone, PartialEq, Eq)]
enum ContainerRowState {
    Group {
        group: docker::ContainerGroup,
        expanded: bool,
        selected: bool,
    },
    Container {
        container: docker::DockerContainer,
        selected: bool,
    },
}

struct RightPane {
    root: gtk::Box,
    title_label: gtk::Label,
    subtitle_label: gtk::Label,
    stack: gtk::Stack,
    status_label: gtk::Label,
    details_box: gtk::Box,
    inspect_view: code_editor::CodeEditor,
}

impl ContainersPage {
    pub(super) fn new(ctx: PageContext) -> Self {
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

    fn refresh(&self, _snapshot: &RepositorySnapshot) {}

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

struct RefreshRequest {
    ctx: PageContext,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
    completion: Option<PageRefreshComplete>,
}

fn refresh_containers(request: RefreshRequest) {
    let generation = request.refresh_generation.get().wrapping_add(1).max(1);
    request.refresh_generation.set(generation);
    request.state.show_loading("Refreshing containers...");
    request.right.show_loading("Refreshing containers...");
    request.tree.clear();

    let Some(docker) = request.ctx.docker() else {
        log::warn!("docker refresh unavailable: missing DockerAccess");
        request
            .state
            .show_empty("dialog-warning-symbolic", "Containers unavailable.");
        request.right.show_error(
            "Containers Error",
            "Docker is unavailable for this workspace.",
        );
        complete_container_refresh(&request.completion);
        return;
    };

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        log::info!("docker list start");
        let result = docker::list_inventory(docker.as_ref());
        if let Err(err) = sender.send(result) {
            log::warn!("docker list response receiver disconnected: {err}");
        }
    });

    glib::timeout_add_local(DOCKER_WORKER_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok(Ok(next_inventory)) => {
                if request.refresh_generation.get() != generation {
                    log::debug!("docker list skipped stale generation={generation}");
                    complete_container_refresh(&request.completion);
                    return glib::ControlFlow::Break;
                }

                log::info!(
                    "docker list success groups={} containers={}",
                    next_inventory.groups.len(),
                    next_inventory.container_count()
                );
                for group in &next_inventory.groups {
                    if group.is_compose() {
                        request
                            .expanded_groups
                            .borrow_mut()
                            .insert(group.key.clone());
                    }
                }
                request.inventory.replace(Some(next_inventory));
                validate_selection(&request.inventory, &request.selected);
                render_container_tree(
                    &request.ctx,
                    &request.tree,
                    &request.state,
                    &request.right,
                    &request.inventory,
                    &request.selected,
                    &request.expanded_groups,
                    &request.refresh_generation,
                    &request.inspect_generation,
                    &request.active_context_menu,
                );
                complete_container_refresh(&request.completion);
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                if request.refresh_generation.get() != generation {
                    complete_container_refresh(&request.completion);
                    return glib::ControlFlow::Break;
                }
                log::warn!("docker list failed: {err}");
                request.inventory.borrow_mut().take();
                request
                    .state
                    .show_empty("dialog-warning-symbolic", "Containers unavailable.");
                request.right.show_error("Containers Error", &err);
                request.tree.clear();
                complete_container_refresh(&request.completion);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("docker list response channel disconnected before completion");
                request
                    .state
                    .show_empty("dialog-warning-symbolic", "Containers unavailable.");
                request.right.show_error(
                    "Containers Error",
                    "Containers refresh worker stopped unexpectedly.",
                );
                complete_container_refresh(&request.completion);
                glib::ControlFlow::Break
            }
        }
    });
}

fn complete_container_refresh(completion: &Option<PageRefreshComplete>) {
    if let Some(completion) = completion {
        completion.as_ref()();
    }
}

fn render_container_tree(
    ctx: &PageContext,
    tree: &Rc<ContainerTreeView>,
    state: &LeftPaneState,
    right: &Rc<RightPane>,
    inventory: &Rc<RefCell<Option<Inventory>>>,
    selected: &Rc<RefCell<Option<Selection>>>,
    expanded_groups: &Rc<RefCell<HashSet<String>>>,
    refresh_generation: &Rc<Cell<u64>>,
    inspect_generation: &Rc<Cell<u64>>,
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
) {
    tree.clear();
    let Some(inventory_value) = inventory.borrow().clone() else {
        state.show_empty("container-symbolic", "Press F5 to refresh Containers.");
        right.show_empty("Press F5 to refresh Containers.");
        return;
    };
    if inventory_value.groups.is_empty() || inventory_value.container_count() == 0 {
        state.show_empty("container-symbolic", "No containers");
        right.show_empty("No containers.");
        return;
    }

    let query = state.search_query.borrow().clone();
    let search_active = !query.trim().is_empty();
    let visible_inventory = if search_active {
        filter_inventory(&inventory_value, &query)
    } else {
        inventory_value.clone()
    };
    if visible_inventory.groups.is_empty() || visible_inventory.container_count() == 0 {
        state.show_empty("system-search-symbolic", "No matching containers.");
        show_selected_details(ctx, right, inventory, selected, inspect_generation);
        return;
    }

    state.show_tree();

    let mut rows = Vec::new();
    for group in &visible_inventory.groups {
        let expanded = search_active || expanded_groups.borrow().contains(&group.key);
        rows.push(TreeRow {
            key: ContainerRowKey::Group(group.key.clone()),
            depth: 0,
            height: tree_view::ICON_ROW_HEIGHT_F64,
            branch: true,
            expanded,
            sticky: true,
            state: ContainerRowState::Group {
                group: group.clone(),
                expanded,
                selected: selected.borrow().as_ref() == Some(&Selection::Group(group.key.clone())),
            },
        });

        if !expanded {
            continue;
        }

        for container in &group.containers {
            rows.push(TreeRow {
                key: ContainerRowKey::Container(container.id.clone()),
                depth: 1,
                height: tree_view::ICON_ROW_HEIGHT_F64,
                branch: false,
                expanded: false,
                sticky: false,
                state: ContainerRowState::Container {
                    container: container.clone(),
                    selected: selected.borrow().as_ref()
                        == Some(&Selection::Container(container.id.clone())),
                },
            });
        }
    }

    let ctx_for_mount = ctx.clone();
    let tree_for_mount = tree.clone();
    let state_for_mount = state.clone();
    let right_for_mount = right.clone();
    let inventory_for_mount = inventory.clone();
    let selected_for_mount = selected.clone();
    let expanded_for_mount = expanded_groups.clone();
    let refresh_for_mount = refresh_generation.clone();
    let inspect_for_mount = inspect_generation.clone();
    let menu_for_mount = active_context_menu.clone();
    let mount = move |_: usize,
                      _: &ContainerRowKey,
                      state: &TreeRenderState<ContainerRowKey, ContainerRowState>| {
        container_tree_row_widget(ContainerRowRequest {
            ctx: ctx_for_mount.clone(),
            tree: tree_for_mount.clone(),
            state: state_for_mount.clone(),
            right: right_for_mount.clone(),
            inventory: inventory_for_mount.clone(),
            selected: selected_for_mount.clone(),
            expanded_groups: expanded_for_mount.clone(),
            refresh_generation: refresh_for_mount.clone(),
            inspect_generation: inspect_for_mount.clone(),
            active_context_menu: menu_for_mount.clone(),
            render: state.clone(),
        })
    };

    let ctx_for_update = ctx.clone();
    let tree_for_update = tree.clone();
    let state_for_update = state.clone();
    let right_for_update = right.clone();
    let inventory_for_update = inventory.clone();
    let selected_for_update = selected.clone();
    let expanded_for_update = expanded_groups.clone();
    let refresh_for_update = refresh_generation.clone();
    let inspect_for_update = inspect_generation.clone();
    let menu_for_update = active_context_menu.clone();
    let update =
        move |_: usize,
              widget: &gtk::Widget,
              previous: &TreeRenderState<ContainerRowKey, ContainerRowState>,
              state: &TreeRenderState<ContainerRowKey, ContainerRowState>| {
            let request = ContainerRowRequest {
                ctx: ctx_for_update.clone(),
                tree: tree_for_update.clone(),
                state: state_for_update.clone(),
                right: right_for_update.clone(),
                inventory: inventory_for_update.clone(),
                selected: selected_for_update.clone(),
                expanded_groups: expanded_for_update.clone(),
                refresh_generation: refresh_for_update.clone(),
                inspect_generation: inspect_for_update.clone(),
                active_context_menu: menu_for_update.clone(),
                render: state.clone(),
            };
            update_container_tree_row_widget(widget, previous, request);
        };
    tree.set_rows(rows, TreeRenderer::new(mount, update));

    show_selected_details(ctx, right, inventory, selected, inspect_generation);
}

#[derive(Clone)]
struct ContainerRowRequest {
    ctx: PageContext,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
    render: TreeRenderState<ContainerRowKey, ContainerRowState>,
}

fn container_tree_row_widget(request: ContainerRowRequest) -> gtk::Widget {
    match request.render.row.state.clone() {
        ContainerRowState::Group {
            group,
            expanded,
            selected,
        } => container_group_tree_row(request, group, expanded, selected),
        ContainerRowState::Container {
            container,
            selected,
        } => container_leaf_tree_row(request, container, selected),
    }
}

fn container_group_tree_row(
    request: ContainerRowRequest,
    group: docker::ContainerGroup,
    expanded: bool,
    selected: bool,
) -> gtk::Widget {
    let key = ContainerRowKey::Group(group.key.clone());
    let disclosure = request.tree.disclosure_widget(key, expanded);
    let icon = gtk::Image::from_icon_name(if group.is_compose() {
        "flatpak-symbolic"
    } else {
        "ui-container-host-symbolic"
    });
    icon.set_pixel_size(tree_view::ICON_SIZE);
    let count = gtk::Label::builder()
        .label(group.containers.len().to_string())
        .css_classes(["dim-label", "numeric", CONTAINER_GROUP_COUNT_CLASS])
        .build();
    let row = IconRow::builder(&group.title)
        .set_icon(icon)
        .depth(request.render.row.depth)
        .selected(selected)
        .sticky(request.render.sticky)
        .bottom_sticky(request.render.bottom)
        .disclosure(disclosure)
        .trailing(count)
        .on_primary_click({
            let request = request.clone();
            let group = group.clone();

            move |_, _, _, _| select_container_group(&request, &group)
        })
        .on_secondary_click({
            let request = request.clone();
            let group = group.clone();

            move |parent, gesture, x, y| {
                request
                    .selected
                    .replace(Some(Selection::Group(group.key.clone())));
                request.right.show_group(&group);
                show_compose_context_menu(
                    &request.ctx,
                    parent,
                    &group,
                    x,
                    y,
                    request.inventory.clone(),
                    request.selected.clone(),
                    request.tree.clone(),
                    request.state.clone(),
                    request.right.clone(),
                    request.expanded_groups.clone(),
                    request.refresh_generation.clone(),
                    request.inspect_generation.clone(),
                    request.active_context_menu.clone(),
                );
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        })
        .build();
    row.root
        .set_size_request(request.render.width, tree_view::ICON_ROW_HEIGHT);
    row.root.upcast()
}

fn container_leaf_tree_row(
    request: ContainerRowRequest,
    container: docker::DockerContainer,
    selected: bool,
) -> gtk::Widget {
    let icon = gtk::Image::from_icon_name(container_state_icon(&container.state));
    icon.set_pixel_size(tree_view::ICON_SIZE);
    let state = gtk::Label::builder()
        .label(&container.state)
        .css_classes(["dim-label", "caption", CONTAINER_STATE_CLASS])
        .build();
    let row = IconRow::builder(&container.name)
        .set_icon(icon)
        .depth(request.render.row.depth)
        .selected(selected)
        .sticky(request.render.sticky)
        .bottom_sticky(request.render.bottom)
        .trailing(state)
        .on_primary_click({
            let request = request.clone();
            let container = container.clone();

            move |_, _, _, _| select_container(&request, &container)
        })
        .on_secondary_click({
            let request = request.clone();
            let container = container.clone();

            move |parent, gesture, x, y| {
                request
                    .selected
                    .replace(Some(Selection::Container(container.id.clone())));
                show_selected_details(
                    &request.ctx,
                    &request.right,
                    &request.inventory,
                    &request.selected,
                    &request.inspect_generation,
                );
                show_container_context_menu(
                    &request.ctx,
                    parent,
                    &container,
                    x,
                    y,
                    request.inventory.clone(),
                    request.selected.clone(),
                    request.tree.clone(),
                    request.state.clone(),
                    request.right.clone(),
                    request.expanded_groups.clone(),
                    request.refresh_generation.clone(),
                    request.inspect_generation.clone(),
                    request.active_context_menu.clone(),
                );
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        })
        .build();
    row.root
        .set_size_request(request.render.width, tree_view::ICON_ROW_HEIGHT);
    row.root.upcast()
}

fn select_container_group(request: &ContainerRowRequest, group: &docker::ContainerGroup) {
    log::debug!("docker group selected key={}", group.key);
    request
        .selected
        .replace(Some(Selection::Group(group.key.clone())));
    if request.state.search_query.borrow().trim().is_empty() {
        if request.expanded_groups.borrow().contains(&group.key) {
            request.expanded_groups.borrow_mut().remove(&group.key);
        } else {
            request
                .expanded_groups
                .borrow_mut()
                .insert(group.key.clone());
        }
    }
    render_container_tree(
        &request.ctx,
        &request.tree,
        &request.state,
        &request.right,
        &request.inventory,
        &request.selected,
        &request.expanded_groups,
        &request.refresh_generation,
        &request.inspect_generation,
        &request.active_context_menu,
    );
}

fn filter_inventory(inventory: &Inventory, query: &str) -> Inventory {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return inventory.clone();
    }

    Inventory {
        groups: inventory
            .groups
            .iter()
            .filter_map(|group| {
                if group_matches(group, &query) {
                    return Some(group.clone());
                }
                let containers = group
                    .containers
                    .iter()
                    .filter(|container| container_matches(container, &query))
                    .cloned()
                    .collect::<Vec<_>>();
                (!containers.is_empty()).then(|| {
                    let mut group = group.clone();
                    group.containers = containers;
                    group
                })
            })
            .collect(),
    }
}

fn group_matches(group: &docker::ContainerGroup, query: &str) -> bool {
    text_matches(query, [&group.title, &group.name, &group.key])
        || group.compose_metadata().is_some_and(|compose| {
            text_matches(
                query,
                [
                    &compose.project,
                    compose.working_dir.as_deref().unwrap_or_default(),
                    compose.environment_file.as_deref().unwrap_or_default(),
                    compose.config_files_raw.as_deref().unwrap_or_default(),
                ],
            ) || compose
                .config_files
                .iter()
                .any(|value| value.to_lowercase().contains(query))
        })
}

fn container_matches(container: &docker::DockerContainer, query: &str) -> bool {
    text_matches(
        query,
        [
            &container.id,
            container.short_id(),
            &container.name,
            &container.image,
            &container.command,
            &container.state,
            &container.status,
            container.service.as_deref().unwrap_or_default(),
            &container.ports,
            &container.ports_raw,
            &container.networks_raw,
            &container.mounts_raw,
            &container.labels_raw,
        ],
    ) || container
        .networks
        .iter()
        .chain(container.mounts.iter())
        .any(|value| value.to_lowercase().contains(query))
        || container.labels.iter().any(|(key, value)| {
            key.to_lowercase().contains(query) || value.to_lowercase().contains(query)
        })
}

fn text_matches<const N: usize>(query: &str, values: [&str; N]) -> bool {
    values
        .into_iter()
        .any(|value| value.to_lowercase().contains(query))
}

fn select_container(request: &ContainerRowRequest, container: &docker::DockerContainer) {
    log::debug!("docker container selected id={}", container.id);
    request
        .selected
        .replace(Some(Selection::Container(container.id.clone())));
    render_container_tree(
        &request.ctx,
        &request.tree,
        &request.state,
        &request.right,
        &request.inventory,
        &request.selected,
        &request.expanded_groups,
        &request.refresh_generation,
        &request.inspect_generation,
        &request.active_context_menu,
    );
}

fn update_container_tree_row_widget(
    widget: &gtk::Widget,
    previous: &TreeRenderState<ContainerRowKey, ContainerRowState>,
    request: ContainerRowRequest,
) {
    if previous.sticky != request.render.sticky
        || previous.bottom != request.render.bottom
        || previous.width != request.render.width
    {
        widget.set_size_request(request.render.width, request.render.row.height as i32);
        tree_view::sync_icon_row_bottom_sticky(widget, request.render.bottom);
    }

    match (&previous.row.state, &request.render.row.state) {
        (
            ContainerRowState::Group {
                group: previous_group,
                expanded: previous_expanded,
                selected: previous_selected,
            },
            ContainerRowState::Group {
                group,
                expanded,
                selected,
            },
        ) => update_container_group_widget(
            widget,
            &request,
            previous_group,
            *previous_expanded,
            *previous_selected,
            group,
            *expanded,
            *selected,
        ),
        (
            ContainerRowState::Container {
                container: previous_container,
                selected: previous_selected,
            },
            ContainerRowState::Container {
                container,
                selected,
            },
        ) => update_container_leaf_widget(
            widget,
            previous_container,
            *previous_selected,
            container,
            *selected,
        ),
        _ => replace_tree_row_widget(widget, container_tree_row_widget(request)),
    }
}

fn update_container_group_widget(
    widget: &gtk::Widget,
    request: &ContainerRowRequest,
    previous_group: &docker::ContainerGroup,
    previous_expanded: bool,
    previous_selected: bool,
    group: &docker::ContainerGroup,
    expanded: bool,
    selected: bool,
) {
    if previous_selected != selected {
        tree_view::sync_icon_row_selected(widget, selected);
    }
    if previous_group.title != group.title
        && let Some(title) = tree_view::icon_row_title(widget)
    {
        title.set_label(&group.title);
    }
    if previous_group.containers.len() != group.containers.len()
        && let Some(title) = tree_view::icon_row_title(widget)
        && let Some(count) = tree_view::icon_row_child_after(&title, CONTAINER_GROUP_COUNT_CLASS)
            .and_then(|widget| widget.downcast::<gtk::Label>().ok())
    {
        count.set_label(&group.containers.len().to_string());
    }
    let icon_name = if group.is_compose() {
        "flatpak-symbolic"
    } else {
        "ui-container-host-symbolic"
    };
    if previous_group.is_compose() != group.is_compose()
        && let Some(icon) =
            tree_view::icon_row_icon(widget).and_then(|widget| widget.downcast::<gtk::Image>().ok())
    {
        icon.set_icon_name(Some(icon_name));
    }
    if previous_expanded != expanded
        && let Some(handle) = tree_view::icon_row_disclosure(widget)
    {
        let key = ContainerRowKey::Group(group.key.clone());
        let should_animate = request.tree.prepare_disclosure(&key, expanded);
        if should_animate {
            request.tree.animate_disclosure(&handle, key);
        } else {
            handle.queue_draw();
        }
    }
}

fn update_container_leaf_widget(
    widget: &gtk::Widget,
    previous_container: &docker::DockerContainer,
    previous_selected: bool,
    container: &docker::DockerContainer,
    selected: bool,
) {
    if previous_selected != selected {
        tree_view::sync_icon_row_selected(widget, selected);
    }
    if previous_container.name != container.name
        && let Some(title) = tree_view::icon_row_title(widget)
    {
        title.set_label(&container.name);
    }
    if previous_container.state != container.state {
        if let Some(icon) =
            tree_view::icon_row_icon(widget).and_then(|widget| widget.downcast::<gtk::Image>().ok())
        {
            icon.set_icon_name(Some(container_state_icon(&container.state)));
        }
        if let Some(title) = tree_view::icon_row_title(widget)
            && let Some(state) = tree_view::icon_row_child_after(&title, CONTAINER_STATE_CLASS)
                .and_then(|widget| widget.downcast::<gtk::Label>().ok())
        {
            state.set_label(&container.state);
        }
    }
}

fn replace_tree_row_widget(existing: &gtk::Widget, next: gtk::Widget) {
    let Some(parent) = existing
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Box>().ok())
    else {
        return;
    };
    next.insert_after(&parent, Some(existing));
    parent.remove(existing);
}

fn show_selected_details(
    ctx: &PageContext,
    right: &Rc<RightPane>,
    inventory: &Rc<RefCell<Option<Inventory>>>,
    selected: &Rc<RefCell<Option<Selection>>>,
    inspect_generation: &Rc<Cell<u64>>,
) {
    let Some(selection) = selected.borrow().clone() else {
        right.show_empty("Select a container or Compose project.");
        return;
    };
    let Some(inventory) = inventory.borrow().clone() else {
        right.show_empty("Press F5 to refresh Containers.");
        return;
    };

    match selection {
        Selection::Group(key) => {
            if let Some(group) = inventory.group_by_key(&key) {
                right.show_group(group);
            } else {
                right.show_empty("The selected container group is no longer available.");
            }
        }
        Selection::Container(id) => {
            if let Some(container) = inventory.container_by_id(&id).cloned() {
                let ctx = ctx.clone();
                let right_for_action = right.clone();
                let inspect_generation = inspect_generation.clone();
                right.show_container(&container.clone(), move || {
                    inspect_container(
                        &ctx,
                        &right_for_action,
                        &inspect_generation,
                        container.clone(),
                    );
                });
            } else {
                right.show_empty("The selected container is no longer available.");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn show_container_context_menu<W: IsA<gtk::Widget>>(
    ctx: &PageContext,
    parent: &W,
    container: &docker::DockerContainer,
    x: f64,
    y: f64,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let sections = vec![
        ActionMenuSection::new(vec![
            ActionMenuItem::new("View Logs", ContainerMenuAction::ViewLogs, true),
            ActionMenuItem::new(
                "Attach Shell",
                ContainerMenuAction::AttachShell,
                docker::state_is_running(&container.state),
            ),
            ActionMenuItem::new("Inspect", ContainerMenuAction::Inspect, true),
        ]),
        ActionMenuSection::new(vec![
            ActionMenuItem::new("Start", ContainerMenuAction::Start, container.can_start()),
            ActionMenuItem::new("Stop", ContainerMenuAction::Stop, container.can_stop()),
            ActionMenuItem::new(
                "Restart",
                ContainerMenuAction::Restart,
                container.can_restart(),
            ),
        ]),
        ActionMenuSection::new(vec![ActionMenuItem::new(
            "Remove",
            ContainerMenuAction::Remove,
            container.can_remove()
                && container
                    .action_enablement()
                    .is_enabled(docker::ContainerAction::Remove),
        )]),
    ];
    let container = container.clone();
    let ctx = ctx.clone();
    let retained_context_menu = active_context_menu.clone();
    let popover = context_menu::popup_action_menu(parent, x, y, sections, move |action| {
        handle_container_menu_action(
            &ctx,
            &right,
            container.clone(),
            action,
            inventory.clone(),
            selected.clone(),
            tree.clone(),
            state.clone(),
            expanded_groups.clone(),
            refresh_generation.clone(),
            inspect_generation.clone(),
            active_context_menu.clone(),
        );
    });
    retain_context_menu(&retained_context_menu, popover.upcast_ref::<gtk::Popover>());
}

#[allow(clippy::too_many_arguments)]
fn handle_container_menu_action(
    ctx: &PageContext,
    right: &Rc<RightPane>,
    container: docker::DockerContainer,
    action: ContainerMenuAction,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    match action {
        ContainerMenuAction::ViewLogs => run_terminal_command(
            ctx,
            docker_logs_command(ctx, &container.id),
            &format!("Logs {}", container.name),
            "Started Docker logs in terminal.",
        ),
        ContainerMenuAction::AttachShell => run_terminal_command(
            ctx,
            docker_shell_command(ctx, &container.id),
            &format!("Shell {}", container.name),
            "Started container shell in terminal.",
        ),
        ContainerMenuAction::Inspect => {
            inspect_container(ctx, right, &inspect_generation, container)
        }
        ContainerMenuAction::Start => run_container_lifecycle(
            ctx,
            docker::ContainerAction::Start,
            container,
            inventory,
            selected,
            tree,
            state,
            right.clone(),
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ContainerMenuAction::Stop => run_container_lifecycle(
            ctx,
            docker::ContainerAction::Stop,
            container,
            inventory,
            selected,
            tree,
            state,
            right.clone(),
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ContainerMenuAction::Restart => run_container_lifecycle(
            ctx,
            docker::ContainerAction::Restart,
            container,
            inventory,
            selected,
            tree,
            state,
            right.clone(),
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ContainerMenuAction::Remove => confirm_remove_container(
            ctx,
            container,
            inventory,
            selected,
            tree,
            state,
            right.clone(),
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn show_compose_context_menu<W: IsA<gtk::Widget>>(
    ctx: &PageContext,
    parent: &W,
    group: &docker::ContainerGroup,
    x: f64,
    y: f64,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let Some(compose) = group.compose_metadata().cloned() else {
        return;
    };
    let sections = vec![
        ActionMenuSection::new(vec![ActionMenuItem::new(
            "Compose Logs",
            ComposeMenuAction::Logs,
            true,
        )]),
        ActionMenuSection::new(vec![
            ActionMenuItem::new("Compose Start", ComposeMenuAction::Start, true),
            ActionMenuItem::new("Compose Stop", ComposeMenuAction::Stop, true),
            ActionMenuItem::new("Compose Restart", ComposeMenuAction::Restart, true),
        ]),
        ActionMenuSection::new(vec![ActionMenuItem::new(
            "Compose Down",
            ComposeMenuAction::Down,
            true,
        )]),
    ];
    let ctx = ctx.clone();
    let retained_context_menu = active_context_menu.clone();
    let popover = context_menu::popup_action_menu(parent, x, y, sections, move |action| {
        handle_compose_menu_action(
            &ctx,
            compose.clone(),
            action,
            inventory.clone(),
            selected.clone(),
            tree.clone(),
            state.clone(),
            right.clone(),
            expanded_groups.clone(),
            refresh_generation.clone(),
            inspect_generation.clone(),
            active_context_menu.clone(),
        );
    });
    retain_context_menu(&retained_context_menu, popover.upcast_ref::<gtk::Popover>());
}

#[allow(clippy::too_many_arguments)]
fn handle_compose_menu_action(
    ctx: &PageContext,
    compose: docker::ComposeProject,
    action: ComposeMenuAction,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    match action {
        ComposeMenuAction::Logs => run_terminal_command(
            ctx,
            compose_terminal_command(ctx, &compose, &["logs", "--tail", "1000", "-f"]),
            &format!("Compose Logs {}", compose.project),
            "Started Compose logs in terminal.",
        ),
        ComposeMenuAction::Start => run_compose_lifecycle(
            ctx,
            docker::ComposeAction::Start,
            compose,
            inventory,
            selected,
            tree,
            state,
            right,
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ComposeMenuAction::Stop => run_compose_lifecycle(
            ctx,
            docker::ComposeAction::Stop,
            compose,
            inventory,
            selected,
            tree,
            state,
            right,
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ComposeMenuAction::Restart => run_compose_lifecycle(
            ctx,
            docker::ComposeAction::Restart,
            compose,
            inventory,
            selected,
            tree,
            state,
            right,
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
        ComposeMenuAction::Down => confirm_compose_down(
            ctx,
            compose,
            inventory,
            selected,
            tree,
            state,
            right,
            expanded_groups,
            refresh_generation,
            inspect_generation,
            active_context_menu,
        ),
    }
}

fn inspect_container(
    ctx: &PageContext,
    right: &Rc<RightPane>,
    inspect_generation: &Rc<Cell<u64>>,
    container: docker::DockerContainer,
) {
    let generation = inspect_generation.get().wrapping_add(1).max(1);
    inspect_generation.set(generation);
    right.show_inspect_loading(&container);
    let Some(docker_access) = ctx.docker() else {
        ctx.show_error(
            "Inspect Failed",
            "Docker is unavailable for this workspace.",
        );
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let container_id = container.id.clone();
    thread::spawn(move || {
        log::info!("docker inspect start container_id={container_id}");
        let result = docker::inspect_container(docker_access.as_ref(), &container_id);
        if let Err(err) = sender.send(result) {
            log::warn!("docker inspect response receiver disconnected: {err}");
        }
    });

    let ctx = ctx.clone();
    let right = right.clone();
    let inspect_generation = inspect_generation.clone();
    glib::timeout_add_local(DOCKER_WORKER_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok(Ok(payload)) => {
                if inspect_generation.get() == generation {
                    log::info!("docker inspect success container_id={}", container.id);
                    right.show_inspect(&container, &payload);
                }
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                if inspect_generation.get() == generation {
                    log::warn!("docker inspect failed container_id={}: {err}", container.id);
                    ctx.show_error("Inspect Failed", &err);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("docker inspect response channel disconnected before completion");
                glib::ControlFlow::Break
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_container_lifecycle(
    ctx: &PageContext,
    action: docker::ContainerAction,
    container: docker::DockerContainer,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let Some(docker_access) = ctx.docker() else {
        ctx.show_error(
            "Docker Action Failed",
            "Docker is unavailable for this workspace.",
        );
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let container_id = container.id.clone();
    thread::spawn(move || {
        log::info!("docker container action start action={action:?} container_id={container_id}");
        let result = docker::run_container_action(docker_access.as_ref(), &container_id, action);
        if let Err(err) = sender.send(result) {
            log::warn!("docker container action response receiver disconnected: {err}");
        }
    });

    let ctx = ctx.clone();
    glib::timeout_add_local(DOCKER_WORKER_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok(Ok(message)) => {
                log::info!(
                    "docker container action success action={action:?} container_id={}",
                    container.id
                );
                ctx.show_toast(&message);
                refresh_containers(RefreshRequest {
                    ctx: ctx.clone(),
                    tree: tree.clone(),
                    state: state.clone(),
                    right: right.clone(),
                    inventory: inventory.clone(),
                    selected: selected.clone(),
                    expanded_groups: expanded_groups.clone(),
                    refresh_generation: refresh_generation.clone(),
                    inspect_generation: inspect_generation.clone(),
                    active_context_menu: active_context_menu.clone(),
                    completion: None,
                });
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                log::warn!(
                    "docker container action failed action={action:?} container_id={}: {err}",
                    container.id
                );
                ctx.show_error("Docker Action Failed", &err);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!(
                    "docker container action response channel disconnected before completion"
                );
                glib::ControlFlow::Break
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_compose_lifecycle(
    ctx: &PageContext,
    action: docker::ComposeAction,
    compose: docker::ComposeProject,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let Some(docker_access) = ctx.docker() else {
        ctx.show_error(
            "Compose Action Failed",
            "Docker is unavailable for this workspace.",
        );
        return;
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn({
        let compose = compose.clone();
        move || {
            log::info!(
                "docker compose action start action={action:?} project={}",
                compose.project
            );
            let result = docker::run_compose_action(docker_access.as_ref(), &compose, action);
            if let Err(err) = sender.send(result) {
                log::warn!("docker compose action response receiver disconnected: {err}");
            }
        }
    });

    let ctx = ctx.clone();
    glib::timeout_add_local(DOCKER_WORKER_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok(Ok(message)) => {
                log::info!(
                    "docker compose action success action={action:?} project={}",
                    compose.project
                );
                ctx.show_toast(&message);
                refresh_containers(RefreshRequest {
                    ctx: ctx.clone(),
                    tree: tree.clone(),
                    state: state.clone(),
                    right: right.clone(),
                    inventory: inventory.clone(),
                    selected: selected.clone(),
                    expanded_groups: expanded_groups.clone(),
                    refresh_generation: refresh_generation.clone(),
                    inspect_generation: inspect_generation.clone(),
                    active_context_menu: active_context_menu.clone(),
                    completion: None,
                });
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                log::warn!(
                    "docker compose action failed action={action:?} project={}: {err}",
                    compose.project
                );
                ctx.show_error("Compose Action Failed", &err);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("docker compose action response channel disconnected before completion");
                glib::ControlFlow::Break
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn confirm_remove_container(
    ctx: &PageContext,
    container: docker::DockerContainer,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let Some(window) = ctx.window() else {
        return;
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Remove Container")
        .body(format!("Remove container \"{}\"?", container.name))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let ctx = ctx.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response.as_str() == "remove" {
            run_container_lifecycle(
                &ctx,
                docker::ContainerAction::Remove,
                container,
                inventory,
                selected,
                tree,
                state,
                right,
                expanded_groups,
                refresh_generation,
                inspect_generation,
                active_context_menu,
            );
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn confirm_compose_down(
    ctx: &PageContext,
    compose: docker::ComposeProject,
    inventory: Rc<RefCell<Option<Inventory>>>,
    selected: Rc<RefCell<Option<Selection>>>,
    tree: Rc<ContainerTreeView>,
    state: LeftPaneState,
    right: Rc<RightPane>,
    expanded_groups: Rc<RefCell<HashSet<String>>>,
    refresh_generation: Rc<Cell<u64>>,
    inspect_generation: Rc<Cell<u64>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
) {
    let Some(window) = ctx.window() else {
        return;
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Compose Down")
        .body(format!(
            "Run docker compose down for \"{}\"?",
            compose.project
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("down", "Compose Down");
    dialog.set_response_appearance("down", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let ctx = ctx.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response.as_str() == "down" {
            run_compose_lifecycle(
                &ctx,
                docker::ComposeAction::Down,
                compose,
                inventory,
                selected,
                tree,
                state,
                right,
                expanded_groups,
                refresh_generation,
                inspect_generation,
                active_context_menu,
            );
        }
    });
}

fn run_terminal_command(
    ctx: &PageContext,
    command: Result<ShellCommandSpec, String>,
    title: &str,
    message: &str,
) {
    log::info!("docker terminal command start title={title}");
    let command = match command {
        Ok(command) => command,
        Err(err) => {
            log::warn!("docker terminal command unavailable title={title}: {err}");
            ctx.show_error("Docker Terminal Failed", &err);
            return;
        }
    };
    match ctx.run_shell_command(&command, title) {
        Ok(()) => ctx.show_toast(message),
        Err(err) => {
            log::warn!("docker terminal command failed title={title}: {err}");
            ctx.show_error("Docker Terminal Failed", &err);
        }
    }
}

fn docker_logs_command(ctx: &PageContext, container_id: &str) -> Result<ShellCommandSpec, String> {
    let Some(docker) = ctx.docker() else {
        return Err("Docker is unavailable for this workspace.".to_string());
    };
    docker.docker_command(
        &["logs", "--tail", "1000", "-f", container_id]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        None,
    )
}

fn docker_shell_command(ctx: &PageContext, container_id: &str) -> Result<ShellCommandSpec, String> {
    let Some(docker) = ctx.docker() else {
        return Err("Docker is unavailable for this workspace.".to_string());
    };
    docker.docker_command(
        &["exec", "-it", container_id, "sh"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        None,
    )
}

fn compose_terminal_command(
    ctx: &PageContext,
    compose: &docker::ComposeProject,
    args: &[&str],
) -> Result<ShellCommandSpec, String> {
    let Some(docker_access) = ctx.docker() else {
        return Err("Docker is unavailable for this workspace.".to_string());
    };
    let working_dir = compose
        .working_dir
        .as_ref()
        .map(|path| WorkspacePath::from_absolute(path.clone()))
        .unwrap_or_else(|| ctx.workspace_ref().root);
    docker_access.docker_command(&docker::compose_args(compose, args), Some(&working_dir))
}

fn validate_selection(
    inventory: &Rc<RefCell<Option<Inventory>>>,
    selected: &Rc<RefCell<Option<Selection>>>,
) {
    let Some(selection) = selected.borrow().clone() else {
        return;
    };
    let Some(inventory) = inventory.borrow().clone() else {
        selected.borrow_mut().take();
        return;
    };

    let valid = match &selection {
        Selection::Group(key) => inventory.group_by_key(key).is_some(),
        Selection::Container(id) => inventory.container_by_id(id).is_some(),
    };
    if !valid {
        log::debug!("docker selection cleared missing selection={selection:?}");
        selected.borrow_mut().take();
    }
}

fn append_section<K, V>(parent: &gtk::Box, title: &str, rows: Vec<(K, V)>)
where
    K: Into<String>,
    V: Into<String>,
{
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .hexpand(true)
        .build();
    section.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );

    for (key, value) in rows {
        let key = key.into();
        let value = value.into();
        section.append(&detail_row(&key, &value));
    }
    parent.append(&section);
}

fn append_map_section(
    parent: &gtk::Box,
    title: &str,
    values: &std::collections::BTreeMap<String, String>,
) {
    let rows = if values.is_empty() {
        vec![("Labels".to_string(), "None".to_string())]
    } else {
        values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>()
    };
    append_section(parent, title, rows);
}

fn detail_row(label: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    row.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .width_request(160)
            .css_classes(["dim-label"])
            .build(),
    );
    row.append(
        &gtk::Label::builder()
            .label(value)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .hexpand(true)
            .build(),
    );
    row
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn display_values(values: &[String]) -> String {
    if values.is_empty() {
        "None".to_string()
    } else {
        values.join(", ")
    }
}

fn display_scalar(value: &str) -> String {
    if value.trim().is_empty() {
        "None".to_string()
    } else {
        value.to_string()
    }
}

fn unique_values<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(value) {
            unique.push(value.clone());
        }
    }
    unique
}

fn unique_scalar_values<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if value.trim().is_empty() {
            continue;
        }
        if !unique.contains(value) {
            unique.push(value.clone());
        }
    }
    unique
}

fn container_state_icon(state: &str) -> &'static str {
    if docker::state_is_running(state) {
        "builder-run-start-symbolic"
    } else {
        "builder-run-stop-symbolic"
    }
}

fn retain_context_menu(
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
    popover: &gtk::Popover,
) {
    if let Some(existing) = active_context_menu.borrow_mut().replace(popover.clone()) {
        existing.popdown();
    }
}
