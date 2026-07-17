use adw::prelude::*;
pub(crate) use craic_ui_core::ui::pages::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

struct LazyChangesPage {
    state: Rc<LazyChangesPageState>,
}

struct LazyChangesPageState {
    ctx: PageContext,
    page: RefCell<Option<Rc<craic_ui_vcs::ChangesPage>>>,
    left: gtk::Box,
    right: gtk::Box,
    pending_snapshot: RefCell<Option<crate::git::WorkspaceSnapshot>>,
    pending_error: RefCell<Option<String>>,
    load_scheduled: Cell<bool>,
}

impl LazyChangesPage {
    fn new(ctx: PageContext) -> Self {
        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        left.append(
            &adw::StatusPage::builder()
                .icon_name("document-edit-symbolic")
                .title("Changes")
                .description("Loading repository changes…")
                .build(),
        );
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        right.append(
            &adw::StatusPage::builder()
                .icon_name("document-edit-symbolic")
                .title("Changes")
                .description("The changes preview will appear shortly.")
                .build(),
        );
        Self {
            state: Rc::new(LazyChangesPageState {
                ctx,
                page: RefCell::new(None),
                left,
                right,
                pending_snapshot: RefCell::new(None),
                pending_error: RefCell::new(None),
                load_scheduled: Cell::new(false),
            }),
        }
    }

    fn schedule_load(&self) {
        if self.state.page.borrow().is_some() || self.state.load_scheduled.replace(true) {
            return;
        }

        log::info!("lazy page load scheduled label=Changes trigger=first-frame");
        let state = self.state.clone();
        if let Some(window) = state.ctx.window() {
            window.add_tick_callback(move |_, _| {
                let state = state.clone();
                gtk::glib::idle_add_local_once(move || Self::load(&state));
                gtk::glib::ControlFlow::Break
            });
        } else {
            gtk::glib::idle_add_local_once(move || Self::load(&state));
        }
    }

    fn load(state: &Rc<LazyChangesPageState>) {
        if state.page.borrow().is_some() {
            return;
        }

        let started = Instant::now();
        let page = Rc::new(craic_ui_vcs::ChangesPage::new(state.ctx.clone()));
        state.page.replace(Some(page.clone()));
        let left_slot = state.left.clone();
        let right_slot = state.right.clone();
        page.initialize(Box::new(move |left, right| {
            while let Some(child) = left_slot.first_child() {
                left_slot.remove(&child);
            }
            while let Some(child) = right_slot.first_child() {
                right_slot.remove(&child);
            }
            left_slot.append(&left);
            right_slot.append(&right);
        }));
        if let Some(snapshot) = state.pending_snapshot.borrow_mut().take() {
            page.refresh(&snapshot, Rc::new(|| {}));
        }
        if let Some(error) = state.pending_error.borrow_mut().take() {
            page.set_error(&error);
        }
        page.activate();
        log::info!(
            "lazy page constructed label=Changes elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
}

impl Page for LazyChangesPage {
    fn label(&self) -> &'static str {
        "Changes"
    }

    fn icon_name(&self) -> &'static str {
        "document-edit-symbolic"
    }

    fn initialize(&self, completion: PageInitializeComplete) {
        completion(
            self.state.left.clone().upcast(),
            self.state.right.clone().upcast(),
        );
    }

    fn activate(&self) {
        if let Some(page) = self.state.page.borrow().as_ref() {
            page.activate();
        } else {
            self.schedule_load();
        }
    }

    fn workspace_changed(&self) {
        self.state.pending_snapshot.borrow_mut().take();
        self.state.pending_error.borrow_mut().take();
        if let Some(page) = self.state.page.borrow().as_ref() {
            page.workspace_changed();
        }
    }

    fn refresh(&self, snapshot: &crate::git::WorkspaceSnapshot, completion: PageRefreshComplete) {
        if let Some(page) = self.state.page.borrow().as_ref() {
            page.refresh(snapshot, completion);
        } else {
            self.state.pending_snapshot.replace(Some(snapshot.clone()));
            self.state.pending_error.borrow_mut().take();
            completion();
        }
    }

    fn refresh_page(&self, completion: PageRefreshComplete) -> PageRefreshRequest {
        if let Some(page) = self.state.page.borrow().as_ref() {
            page.refresh_page(completion)
        } else {
            self.schedule_load();
            completion();
            PageRefreshRequest::Custom
        }
    }

    fn set_error(&self, message: &str) {
        if let Some(page) = self.state.page.borrow().as_ref() {
            page.set_error(message);
        } else {
            self.state.pending_snapshot.borrow_mut().take();
            self.state.pending_error.replace(Some(message.to_string()));
        }
    }

    fn badge(&self) -> Option<PageBadge> {
        self.state
            .page
            .borrow()
            .as_ref()
            .and_then(|page| page.badge())
    }

    fn toggle_left_search(&self) -> bool {
        self.state
            .page
            .borrow()
            .as_ref()
            .is_some_and(|page| page.toggle_left_search())
    }

    fn toggle_right_search(&self) -> bool {
        self.state
            .page
            .borrow()
            .as_ref()
            .is_some_and(|page| page.toggle_right_search())
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        self.state
            .page
            .borrow()
            .as_ref()
            .map_or(PageCommandResult::Ignored, |page| {
                page.handle_command(command)
            })
    }
}

struct DeferredPage {
    label: &'static str,
    icon_name: &'static str,
    factory: RefCell<Option<Box<dyn FnOnce() -> PageRef>>>,
    page: RefCell<Option<PageRef>>,
    pending_snapshot: RefCell<Option<crate::git::WorkspaceSnapshot>>,
    pending_error: RefCell<Option<String>>,
    handles_command: fn(&PageCommand) -> bool,
}

impl DeferredPage {
    fn new(
        label: &'static str,
        icon_name: &'static str,
        factory: impl FnOnce() -> PageRef + 'static,
        handles_command: fn(&PageCommand) -> bool,
    ) -> Self {
        Self {
            label,
            icon_name,
            factory: RefCell::new(Some(Box::new(factory))),
            page: RefCell::new(None),
            pending_snapshot: RefCell::new(None),
            pending_error: RefCell::new(None),
            handles_command,
        }
    }

    fn page(&self) -> PageRef {
        if let Some(page) = self.page.borrow().as_ref() {
            return page.clone();
        }

        let started = Instant::now();
        let page = self
            .factory
            .borrow_mut()
            .take()
            .expect("deferred page factory missing")();
        self.page.replace(Some(page.clone()));
        if let Some(snapshot) = self.pending_snapshot.borrow_mut().take() {
            page.refresh(&snapshot, Rc::new(|| {}));
        }
        if let Some(error) = self.pending_error.borrow_mut().take() {
            page.set_error(&error);
        }
        log::info!(
            "lazy page constructed label={} elapsed_ms={}",
            self.label,
            started.elapsed().as_millis()
        );
        page
    }
}

impl Page for DeferredPage {
    fn label(&self) -> &'static str {
        self.label
    }

    fn icon_name(&self) -> &'static str {
        self.icon_name
    }

    fn initialize(&self, completion: PageInitializeComplete) {
        self.page().initialize(completion);
    }

    fn activate(&self) {
        self.page().activate();
    }

    fn workspace_changed(&self) {
        self.pending_snapshot.borrow_mut().take();
        self.pending_error.borrow_mut().take();
        if let Some(page) = self.page.borrow().as_ref() {
            page.workspace_changed();
        }
    }

    fn refresh(&self, snapshot: &crate::git::WorkspaceSnapshot, completion: PageRefreshComplete) {
        if let Some(page) = self.page.borrow().as_ref() {
            page.refresh(snapshot, completion);
        } else {
            self.pending_snapshot.replace(Some(snapshot.clone()));
            self.pending_error.borrow_mut().take();
            completion();
        }
    }

    fn refresh_page(&self, completion: PageRefreshComplete) -> PageRefreshRequest {
        self.page().refresh_page(completion)
    }

    fn set_error(&self, message: &str) {
        if let Some(page) = self.page.borrow().as_ref() {
            page.set_error(message);
        } else {
            self.pending_snapshot.borrow_mut().take();
            self.pending_error.replace(Some(message.to_string()));
        }
    }

    fn toggle_left_search(&self) -> bool {
        self.page().toggle_left_search()
    }

    fn toggle_right_search(&self) -> bool {
        self.page().toggle_right_search()
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        if self.page.borrow().is_none() && !(self.handles_command)(command) {
            return PageCommandResult::Ignored;
        }
        self.page().handle_command(command)
    }
}

pub(crate) fn build_pages(ctx: PageContext) -> Vec<PageRef> {
    let changes: PageRef = Rc::new(LazyChangesPage::new(ctx.clone()));
    log::info!("startup page deferred label=Changes");

    let history_ctx = ctx.clone();
    let history: PageRef = Rc::new(DeferredPage::new(
        "History",
        "document-open-recent-symbolic",
        move || Rc::new(craic_ui_vcs::HistoryPage::new(history_ctx)),
        |command| matches!(command, PageCommand::OpenCommit(_)),
    ));
    log::info!("startup page deferred label=History");

    let files_ctx = ctx.clone();
    let files: PageRef = Rc::new(DeferredPage::new(
        "Files",
        "code-symbolic",
        move || Rc::new(craic_ui_file::FilePage::new(files_ctx)),
        |command| {
            matches!(
                command,
                PageCommand::OpenSearchMatch { .. } | PageCommand::OpenFileLocation { .. }
            )
        },
    ));
    log::info!("startup page deferred label=Files");

    let containers_ctx = ctx.clone();
    let containers: PageRef = Rc::new(DeferredPage::new(
        "Containers",
        "container-symbolic",
        move || Rc::new(craic_ui_containers::ContainersPage::new(containers_ctx)),
        |_| false,
    ));
    log::info!("startup page deferred label=Containers");

    let step = Instant::now();
    let agents: PageRef = Rc::new(craic_ui_agent::AgentPage::new(ctx));
    log::info!(
        "startup page constructed label=Agents elapsed_ms={}",
        step.elapsed().as_millis()
    );

    vec![changes, history, files, containers, agents]
}
