mod left;
mod provider;
mod right;

use super::{Page, PageCommand, PageCommandResult, PageContext};
use crate::git::RepositorySnapshot;
use crate::gitignore;
use crate::system::capabilities::docker::ComposeFileAction;
use crate::system::capabilities::files::{
    FileAccess, FileKind, FileMetadata, FileWatchCallback, FileWatchChanges, FileWatchRequest,
    FileWatchSubscription,
};
use crate::system::path::ProviderKind;
use crate::system::{SystemPath, WorkspacePath, WorkspaceRef};
use crate::ui::content::code_editor;
use crate::ui::file_type;
use crate::ui::sidebar::file_browser::ContainerFileAction;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const MAX_EDITOR_FILE_BYTES: u64 = 1024 * 1024;
const CODE_FILE_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(75);
const CODE_FILE_REFRESH_DEBOUNCE: Duration = Duration::from_millis(120);
const LIVE_PREVIEW_REFRESH_DEBOUNCE: Duration = Duration::from_millis(60);

pub(super) struct CodePage {
    ctx: PageContext,
    left: left::LeftPane,
    right: Rc<right::RightPane>,
    pending_save: PendingSaveState,
    file_monitor: OpenedFileMonitorState,
    displayed_preview: DisplayedPreviewState,
    skip_next_active_selection: Rc<Cell<bool>>,
}

type PendingSaveState = Rc<RefCell<Option<PendingSave>>>;
type OpenedFileMonitorState = Rc<OpenedFileMonitor>;
type DisplayedPreviewState = Rc<RefCell<Option<DisplayedPreview>>>;

#[derive(Clone)]
struct PendingSave {
    path: String,
    generation: u64,
    base_signature: Option<provider::DiskSignature>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayedPreview {
    system_path: SystemPath,
    signature: PreviewSignature,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewSignature {
    Disk(provider::DiskSignature),
    Content(provider::ContentSignature),
}

#[derive(Clone)]
struct OpenedFileMonitorTarget {
    workspace: WorkspaceRef,
    workspace_path: WorkspacePath,
    file_path: String,
    local_path: Option<PathBuf>,
    signature: Option<provider::DiskSignature>,
}

struct OpenedFileMonitor {
    target: RefCell<Option<OpenedFileMonitorTarget>>,
    subscription: RefCell<Option<FileWatchSubscription>>,
    event_source: RefCell<Option<glib::SourceId>>,
    debounce_source: RefCell<Option<glib::SourceId>>,
    generation: Cell<u64>,
    displayed_preview: DisplayedPreviewState,
}

impl OpenedFileMonitorTarget {
    fn matches(
        &self,
        workspace: &WorkspaceRef,
        workspace_path: &WorkspacePath,
        local_path: Option<&Path>,
    ) -> bool {
        self.workspace == *workspace
            && self.workspace_path == *workspace_path
            && self.local_path.as_deref() == local_path
    }

    fn matches_selected_path(
        &self,
        workspace: &WorkspaceRef,
        workspace_path: &WorkspacePath,
    ) -> bool {
        self.workspace == *workspace && self.workspace_path == *workspace_path
    }
}

impl OpenedFileMonitor {
    fn new(displayed_preview: DisplayedPreviewState) -> OpenedFileMonitorState {
        Rc::new(Self {
            target: RefCell::new(None),
            subscription: RefCell::new(None),
            event_source: RefCell::new(None),
            debounce_source: RefCell::new(None),
            generation: Cell::new(0),
            displayed_preview,
        })
    }

    fn watch_file(
        self: &Rc<Self>,
        ctx: &PageContext,
        right: &Rc<right::RightPane>,
        pending_save: &PendingSaveState,
        workspace: &WorkspaceRef,
        workspace_path: &WorkspacePath,
        file_path: &str,
        local_path: Option<&Path>,
        signature: Option<provider::DiskSignature>,
    ) {
        if self
            .target
            .borrow()
            .as_ref()
            .is_some_and(|target| target.matches(workspace, workspace_path, local_path))
        {
            if let Some(target) = self.target.borrow_mut().as_mut() {
                target.signature = signature;
            }
            return;
        }

        self.stop();

        let target = OpenedFileMonitorTarget {
            workspace: workspace.clone(),
            workspace_path: workspace_path.clone(),
            file_path: file_path.to_string(),
            local_path: local_path.map(Path::to_path_buf),
            signature,
        };
        self.target.replace(Some(target));

        let generation = self.next_generation();
        let Some(files) = ctx.files() else {
            log::info!(
                "opened-file monitor unavailable file_path={file_path} reason=no-file-capability"
            );
            return;
        };

        let request = FileWatchRequest {
            paths: vec![workspace_path.clone()],
            recursive: false,
        };
        let (sender, receiver) = mpsc::channel();
        let sender = Arc::new(Mutex::new(sender));
        let callback: FileWatchCallback = Arc::new(move |changes| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(changes);
            }
        });
        let subscription = match files.watch(request, callback) {
            Ok(subscription) => subscription,
            Err(err) => {
                log::warn!(
                    "opened-file monitor unavailable file_path={} reason={}",
                    file_path,
                    err
                );
                return;
            }
        };

        let monitor_state = Rc::clone(self);
        let ctx = ctx.clone();
        let right = Rc::clone(right);
        let pending_save = pending_save.clone();
        let watched_path = workspace_path.clone();
        let source_id = glib::timeout_add_local(CODE_FILE_EVENT_POLL_INTERVAL, move || {
            if monitor_state.generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            let mut should_reload = false;
            while let Ok(changes) = receiver.try_recv() {
                if file_watch_changes_include_path(&changes, &watched_path) {
                    should_reload = true;
                }
            }

            if should_reload {
                monitor_state.queue_reload(&ctx, &right, &pending_save, generation);
            }

            glib::ControlFlow::Continue
        });
        self.subscription.replace(Some(subscription));
        self.event_source.replace(Some(source_id));
    }

    fn mark_missing(&self, workspace: &WorkspaceRef, workspace_path: &WorkspacePath) {
        let keep_target = self
            .target
            .borrow()
            .as_ref()
            .is_some_and(|target| target.matches_selected_path(workspace, workspace_path));
        if !keep_target {
            self.stop();
            return;
        }

        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.signature = None;
        }
    }

    fn stop_if_workspace_changed(&self, workspace: &WorkspaceRef) {
        let repo_changed = self
            .target
            .borrow()
            .as_ref()
            .is_some_and(|target| target.workspace != *workspace);
        if repo_changed {
            self.stop();
        }
    }

    fn stop(&self) {
        self.next_generation();
        if let Some(source_id) = self.debounce_source.borrow_mut().take() {
            source_id.remove();
        }
        if let Some(source_id) = self.event_source.borrow_mut().take() {
            source_id.remove();
        }
        self.subscription.borrow_mut().take();
        self.target.borrow_mut().take();
    }

    fn next_generation(&self) -> u64 {
        let generation = self.generation.get().wrapping_add(1).max(1);
        self.generation.set(generation);
        generation
    }

    fn queue_reload(
        self: &Rc<Self>,
        ctx: &PageContext,
        right: &Rc<right::RightPane>,
        pending_save: &PendingSaveState,
        generation: u64,
    ) {
        if self.generation.get() != generation || self.debounce_source.borrow().is_some() {
            return;
        }

        let monitor_state = Rc::clone(self);
        let ctx = ctx.clone();
        let right = Rc::clone(right);
        let pending_save = pending_save.clone();
        let source_id = glib::timeout_add_local(CODE_FILE_REFRESH_DEBOUNCE, move || {
            monitor_state.debounce_source.borrow_mut().take();
            monitor_state.reload_if_changed(&ctx, &right, &pending_save, generation);
            glib::ControlFlow::Break
        });
        self.debounce_source.replace(Some(source_id));
    }

    fn reload_if_changed(
        self: &Rc<Self>,
        ctx: &PageContext,
        right: &Rc<right::RightPane>,
        pending_save: &PendingSaveState,
        generation: u64,
    ) {
        if self.generation.get() != generation {
            return;
        }

        let Some(target) = self.target.borrow().clone() else {
            return;
        };
        let workspace = ctx.workspace_ref();
        if target.workspace != workspace {
            return;
        }

        let current_signature = disk_signature_for_path(ctx, &target.file_path)
            .ok()
            .flatten();
        if current_signature == target.signature {
            return;
        }

        if let Some(active_target) = self.target.borrow_mut().as_mut() {
            if active_target.matches(
                &target.workspace,
                &target.workspace_path,
                target.local_path.as_deref(),
            ) {
                active_target.signature = current_signature;
            }
        }

        show_repository_browser_path(
            ctx,
            right,
            pending_save,
            self,
            &self.displayed_preview,
            &target.file_path,
        );
    }
}

impl CodePage {
    pub(super) fn new(ctx: PageContext) -> Self {
        let left = left::LeftPane::new(ctx.files(), ctx.git());
        if let Some(file_browser) = &left.file_browser {
            file_browser.set_opener(ctx.opener());
            file_browser.set_terminal_actions_available(ctx.shell().is_some());
            file_browser.set_container_actions_available(ctx.docker().is_some());
        }
        let right = Rc::new(right::RightPane::new());
        let pending_save: PendingSaveState = Rc::new(RefCell::new(None));
        let displayed_preview: DisplayedPreviewState = Rc::new(RefCell::new(None));
        let file_monitor = OpenedFileMonitor::new(displayed_preview.clone());
        let skip_next_active_selection = Rc::new(Cell::new(false));

        if let Some(file_browser) = &left.file_browser {
            file_browser.connect_selected({
                let ctx = ctx.clone();
                let right = right.clone();
                let pending_save = pending_save.clone();
                let file_monitor = file_monitor.clone();
                let displayed_preview = displayed_preview.clone();

                move |file_path| {
                    show_repository_browser_path(
                        &ctx,
                        &right,
                        &pending_save,
                        &file_monitor,
                        &displayed_preview,
                        &file_path,
                    );
                }
            });

            file_browser.connect_search_match_selected({
                let ctx = ctx.clone();

                move |file_path, start, end| {
                    ctx.dispatch_command(PageCommand::OpenSearchMatch {
                        path: file_path,
                        start,
                        end,
                    });
                }
            });

            file_browser.connect_open_terminal_requested({
                let ctx = ctx.clone();

                move |file_path, is_dir| {
                    let working_dir =
                        browser_terminal_dir(&ctx.workspace_ref().root, &file_path, is_dir);
                    if let Err(err) = ctx.open_terminal(&working_dir) {
                        ctx.show_error("Open Terminal Failed", &err);
                    }
                }
            });

            file_browser.connect_run_in_terminal_requested({
                let ctx = ctx.clone();

                move |file_path| {
                    if let Err(err) = run_repository_file_in_terminal(&ctx, &file_path) {
                        ctx.show_error("Run Failed", &err);
                    }
                }
            });

            file_browser.connect_add_to_chat_requested({
                let ctx = ctx.clone();

                move |file_path| {
                    ctx.dispatch_command(PageCommand::AddFileToAgent(file_path));
                }
            });

            file_browser.connect_ignore_requested({
                let ctx = ctx.clone();

                move |pattern| add_gitignore_pattern(&ctx, &pattern)
            });

            file_browser.connect_container_file_action_requested({
                let ctx = ctx.clone();

                move |file_path, action| run_container_file_action(&ctx, &file_path, action)
            });

            file_browser.connect_open_failed({
                let ctx = ctx.clone();

                move |message| {
                    ctx.show_toast(&message);
                }
            });
        }

        right.file_editor.connect_edit({
            let ctx = ctx.clone();
            let file_editor_path = right.file_editor_path.clone();
            let file_editor_disk_signature = right.file_editor_disk_signature.clone();
            let file_editor = right.file_editor.clone();
            let right = right.clone();
            let pending_save = pending_save.clone();
            let file_monitor = file_monitor.clone();
            let displayed_preview = displayed_preview.clone();
            let save_generation = Rc::new(Cell::new(0_u64));
            let preview_generation = Rc::new(Cell::new(0_u64));

            move || {
                let Some(file_path) = file_editor_path.borrow().clone() else {
                    return;
                };
                let generation = save_generation.get().wrapping_add(1).max(1);
                save_generation.set(generation);
                pending_save.replace(Some(PendingSave {
                    path: file_path.clone(),
                    generation,
                    base_signature: *file_editor_disk_signature.borrow(),
                }));

                let preview_generation_value = preview_generation.get().wrapping_add(1).max(1);
                preview_generation.set(preview_generation_value);
                {
                    let ctx = ctx.clone();
                    let right = right.clone();
                    let file_editor = file_editor.clone();
                    let file_editor_path = file_editor_path.clone();
                    let preview_file_path = file_path.clone();
                    let displayed_preview = displayed_preview.clone();
                    let preview_generation = preview_generation.clone();
                    gtk::glib::timeout_add_local_once(LIVE_PREVIEW_REFRESH_DEBOUNCE, move || {
                        if preview_generation.get() != preview_generation_value {
                            return;
                        }
                        if file_editor_path.borrow().as_deref() != Some(preview_file_path.as_str())
                        {
                            return;
                        }

                        let text = file_editor.document_text();
                        refresh_live_file_preview(
                            &ctx,
                            &right,
                            &displayed_preview,
                            &preview_file_path,
                            &text,
                        );
                    });
                }

                let ctx = ctx.clone();
                let file_editor = file_editor.clone();
                let file_editor_disk_signature = file_editor_disk_signature.clone();
                let file_editor_path = file_editor_path.clone();
                let right = right.clone();
                let pending_save = pending_save.clone();
                let file_monitor = file_monitor.clone();
                let displayed_preview = displayed_preview.clone();
                let save_generation = save_generation.clone();
                gtk::glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    if save_generation.get() != generation {
                        return;
                    }
                    if !pending_save_matches(&pending_save, &file_path, generation) {
                        return;
                    }
                    if file_editor_path.borrow().as_deref() != Some(file_path.as_str()) {
                        return;
                    }
                    let current_signature =
                        disk_signature_for_path(&ctx, &file_path).ok().flatten();
                    let Some(pending) = pending_save.borrow().clone() else {
                        return;
                    };
                    if pending.base_signature != current_signature {
                        clear_pending_save(&pending_save, &file_path, generation);
                        show_repository_browser_path(
                            &ctx,
                            &right,
                            &pending_save,
                            &file_monitor,
                            &displayed_preview,
                            &file_path,
                        );
                        return;
                    }
                    let text = file_editor.document_text();
                    spellcheck_editor_document(&ctx, &file_editor, &file_path, &text);
                    if let Err(err) = write_repository_file(&ctx, &file_path, &text) {
                        ctx.show_error("Save Failed", &err);
                        return;
                    }
                    if let Ok(signature) = disk_signature_for_path(&ctx, &file_path) {
                        file_editor_disk_signature.replace(signature);
                    }
                    clear_pending_save(&pending_save, &file_path, generation);
                });
            }
        });

        if left.file_browser.is_some() {
            show_repository_browser_path(
                &ctx,
                &right,
                &pending_save,
                &file_monitor,
                &displayed_preview,
                "",
            );
        } else {
            right.show_unavailable("Files", "Files are unavailable for this workspace.");
        }

        Self {
            ctx,
            left,
            right,
            pending_save,
            file_monitor,
            displayed_preview,
            skip_next_active_selection,
        }
    }

    fn show_active_selection(&self) {
        if self.skip_next_active_selection.replace(false) {
            log::debug!("skipped active file browser selection replay after command open");
            return;
        }
        if let Some(file_browser) = &self.left.file_browser {
            let selected_path = file_browser.selected_file_path();
            show_repository_browser_path(
                &self.ctx,
                &self.right,
                &self.pending_save,
                &self.file_monitor,
                &self.displayed_preview,
                &selected_path,
            );
        }
    }
}

fn skip_next_active_selection_once(skip_next_active_selection: &Rc<Cell<bool>>) {
    skip_next_active_selection.set(true);
    let skip_next_active_selection = skip_next_active_selection.clone();
    glib::idle_add_local_once(move || {
        skip_next_active_selection.set(false);
    });
}

impl Page for CodePage {
    fn label(&self) -> &'static str {
        "Files"
    }

    fn icon_name(&self) -> &'static str {
        "code-symbolic"
    }

    fn left(&self) -> gtk::Widget {
        self.left.root.clone().upcast()
    }

    fn right(&self) -> gtk::Widget {
        self.right.root.clone().upcast()
    }

    fn activate(&self) {
        self.show_active_selection();
    }

    fn refresh(&self, snapshot: &RepositorySnapshot) {
        let workspace = self.ctx.workspace_ref();
        self.file_monitor.stop_if_workspace_changed(&workspace);
        clear_displayed_preview_if_workspace_changed(&self.displayed_preview, &workspace);
        let Some(file_browser) = &self.left.file_browser else {
            self.right
                .show_unavailable("Files", "Files are unavailable for this workspace.");
            return;
        };
        if let Some(file_access) = self.ctx.files() {
            file_browser.set_opener(self.ctx.opener());
            file_browser.set_terminal_actions_available(self.ctx.shell().is_some());
            file_browser.set_container_actions_available(self.ctx.docker().is_some());
            file_browser.refresh(Some(&snapshot.changed_files), file_access, self.ctx.git());
        } else {
            file_browser.set_opener(None);
            file_browser.set_terminal_actions_available(false);
            file_browser.set_container_actions_available(false);
            self.right
                .show_unavailable("Files", "Files are unavailable for this workspace.");
        }
    }

    fn set_error(&self, message: &str) {
        self.file_monitor.stop();
        self.displayed_preview.borrow_mut().take();
        self.right.show_unavailable("Workspace", message);
    }

    fn toggle_left_search(&self) -> bool {
        let Some(file_browser) = &self.left.file_browser else {
            return false;
        };
        file_browser.toggle_search();
        true
    }

    fn toggle_right_search(&self) -> bool {
        self.right.file_editor.toggle_search();
        true
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        match command {
            PageCommand::OpenSearchMatch { path, start, end } => {
                show_repository_file_match(
                    &self.ctx,
                    &self.right,
                    &self.pending_save,
                    &self.file_monitor,
                    &self.displayed_preview,
                    path,
                    *start,
                    *end,
                );
                PageCommandResult::HandledAndActivate
            }
            PageCommand::OpenFileLocation { path, line, column } => {
                if let Some(file_browser) = &self.left.file_browser {
                    file_browser.reveal_workspace_path(path);
                }
                show_repository_file_location(
                    &self.ctx,
                    &self.right,
                    &self.pending_save,
                    &self.file_monitor,
                    &self.displayed_preview,
                    path,
                    *line,
                    *column,
                );
                skip_next_active_selection_once(&self.skip_next_active_selection);
                PageCommandResult::HandledAndActivate
            }
            PageCommand::ClearSelection => {
                show_repository_browser_path(
                    &self.ctx,
                    &self.right,
                    &self.pending_save,
                    &self.file_monitor,
                    &self.displayed_preview,
                    "",
                );
                PageCommandResult::Handled
            }
            PageCommand::AddFileToAgent(_)
            | PageCommand::OpenAgentSession(_)
            | PageCommand::OpenCommit(_) => PageCommandResult::Ignored,
        }
    }
}

fn show_repository_file_location(
    ctx: &PageContext,
    right: &Rc<right::RightPane>,
    pending_save: &PendingSaveState,
    file_monitor: &OpenedFileMonitorState,
    displayed_preview: &DisplayedPreviewState,
    file_path: &str,
    line: Option<usize>,
    column: Option<usize>,
) {
    if right.file_editor_path.borrow().as_deref() == Some(file_path) {
        let loaded_signature = *right.file_editor_disk_signature.borrow();
        let current_signature = disk_signature_for_path(ctx, file_path).ok().flatten();
        if current_signature == loaded_signature {
            if let Some(line) = line {
                right
                    .file_editor
                    .select_line_column(line, column.unwrap_or(1));
            }
            return;
        }
        if pending_save_path(pending_save).as_deref() == Some(file_path) {
            pending_save.borrow_mut().take();
        }
    }
    if !flush_pending_save(ctx, right, pending_save) {
        return;
    }

    let item = match repository_item(ctx, file_path) {
        Ok(item) => item,
        Err(err) => {
            let workspace = ctx.workspace_ref();
            let workspace_path = workspace.path(file_path);
            file_monitor.mark_missing(&workspace, &workspace_path);
            displayed_preview.borrow_mut().take();
            right.show_unavailable(file_path, &format!("Unable to preview item: {err}"));
            return;
        }
    };
    let selection = line
        .and_then(|line| repository_item_line_column_selection(&item, line, column.unwrap_or(1)));

    show_repository_item(
        ctx,
        right,
        pending_save,
        file_monitor,
        displayed_preview,
        item,
        selection,
    );
}

fn show_repository_browser_path(
    ctx: &PageContext,
    right: &Rc<right::RightPane>,
    pending_save: &PendingSaveState,
    file_monitor: &OpenedFileMonitorState,
    displayed_preview: &DisplayedPreviewState,
    file_path: &str,
) {
    if right.file_editor_path.borrow().as_deref() == Some(file_path) {
        let loaded_signature = *right.file_editor_disk_signature.borrow();
        let current_signature = disk_signature_for_path(ctx, file_path).ok().flatten();
        if current_signature == loaded_signature {
            return;
        }
        if pending_save_path(pending_save).as_deref() == Some(file_path) {
            pending_save.borrow_mut().take();
        }
    }
    if !flush_pending_save(ctx, right, pending_save) {
        return;
    }

    let item = match repository_item(ctx, file_path) {
        Ok(item) => item,
        Err(err) => {
            let workspace = ctx.workspace_ref();
            let workspace_path = workspace.path(file_path);
            file_monitor.mark_missing(&workspace, &workspace_path);
            displayed_preview.borrow_mut().take();
            right.show_unavailable(file_path, &format!("Unable to preview item: {err}"));
            return;
        }
    };

    show_repository_item(
        ctx,
        right,
        pending_save,
        file_monitor,
        displayed_preview,
        item,
        None,
    );
}

fn show_repository_file_match(
    ctx: &PageContext,
    right: &Rc<right::RightPane>,
    pending_save: &PendingSaveState,
    file_monitor: &OpenedFileMonitorState,
    displayed_preview: &DisplayedPreviewState,
    file_path: &str,
    start: usize,
    end: usize,
) {
    if right.file_editor_path.borrow().as_deref() == Some(file_path) {
        let loaded_signature = *right.file_editor_disk_signature.borrow();
        let current_signature = disk_signature_for_path(ctx, file_path).ok().flatten();
        if current_signature == loaded_signature {
            right.file_editor.select_range(start, end);
            return;
        }
        if pending_save_path(pending_save).as_deref() == Some(file_path) {
            pending_save.borrow_mut().take();
        }
    }
    if !flush_pending_save(ctx, right, pending_save) {
        return;
    }

    let item = match repository_item(ctx, file_path) {
        Ok(item) => item,
        Err(err) => {
            let workspace = ctx.workspace_ref();
            let workspace_path = workspace.path(file_path);
            file_monitor.mark_missing(&workspace, &workspace_path);
            displayed_preview.borrow_mut().take();
            right.show_unavailable(file_path, &format!("Unable to preview item: {err}"));
            return;
        }
    };

    show_repository_item(
        ctx,
        right,
        pending_save,
        file_monitor,
        displayed_preview,
        item,
        Some((start, end)),
    );
}

fn repository_item_line_column_selection(
    item: &RepositoryItem,
    line: usize,
    column: usize,
) -> Option<(usize, usize)> {
    if item.metadata.kind != FileKind::File {
        return None;
    }

    let text = match item.prefetched_bytes.clone() {
        Some(bytes) => text_from_repository_bytes(bytes),
        None => read_repository_file(item.files.as_ref(), &item.workspace_path),
    };
    let text = match text {
        Ok(text) => text,
        Err(err) => {
            log::warn!(
                "repository file location selection skipped file_path={} line={} column={}: {}",
                item.workspace_path.relative_or_empty(),
                line,
                column,
                err
            );
            return None;
        }
    };
    let offset = code_editor::byte_offset_for_line_column(&text, line, column);
    Some((offset, offset))
}

fn refresh_live_file_preview(
    ctx: &PageContext,
    right: &right::RightPane,
    displayed_preview: &DisplayedPreviewState,
    file_path: &str,
    text: &str,
) {
    let preview_kind = file_type::preview_kind_for_path(file_path, false);
    let signature = provider::content_signature(text.as_bytes());
    match preview_kind {
        file_type::PreviewKind::Markdown => {
            let html = provider::markdown::markdown_to_html(text);
            let local_path = local_workspace_path(ctx, file_path);
            right
                .file_markdown_preview
                .set_markdown_html(&html, signature, local_path.as_deref());
            right
                .file_markdown_preview
                .set_source_offset(right.file_editor.source_offset_at_scroll_top());
            right
                .file_view_split
                .set_end_child(Some(&right.file_markdown_preview.root));
            set_live_displayed_preview(ctx, displayed_preview, file_path, signature);
            log::debug!("refreshed live markdown preview file_path={file_path}");
        }
        file_type::PreviewKind::Svg => {
            right.file_svg_preview.set_svg(text.as_bytes(), signature);
            right
                .file_view_split
                .set_end_child(Some(&right.file_svg_preview.root));
            set_live_displayed_preview(ctx, displayed_preview, file_path, signature);
            log::debug!("refreshed live svg preview file_path={file_path}");
        }
        _ => {}
    }
}

fn set_live_displayed_preview(
    ctx: &PageContext,
    displayed_preview: &DisplayedPreviewState,
    file_path: &str,
    signature: provider::ContentSignature,
) {
    let path = ctx.workspace_ref().path(file_path);
    let system_path = SystemPath {
        system: ctx.system_ref(),
        workspace: ctx.workspace_ref(),
        path,
    };
    displayed_preview.replace(Some(DisplayedPreview {
        system_path,
        signature: PreviewSignature::Content(signature),
    }));
}

fn local_workspace_path(ctx: &PageContext, file_path: &str) -> Option<PathBuf> {
    (ctx.system_ref().provider_kind == ProviderKind::Local)
        .then(|| PathBuf::from(ctx.workspace_ref().path(file_path).absolute))
}

struct RepositoryItem {
    files: Arc<dyn FileAccess>,
    workspace: WorkspaceRef,
    workspace_path: WorkspacePath,
    local_path: Option<PathBuf>,
    metadata: FileMetadata,
    prefetched_bytes: Option<Vec<u8>>,
}

fn repository_item(ctx: &PageContext, file_path: &str) -> Result<RepositoryItem, String> {
    let files = ctx
        .files()
        .ok_or_else(|| "File access is unavailable for this workspace.".to_string())?;
    let workspace = ctx.workspace_ref();
    let workspace_path = workspace.path(file_path);
    let read = files.read_with_metadata(&workspace_path, Some(MAX_EDITOR_FILE_BYTES))?;
    let metadata = read.metadata;
    let local_path = local_path_for_system_path(&metadata.path);
    Ok(RepositoryItem {
        files,
        workspace,
        workspace_path,
        local_path,
        metadata,
        prefetched_bytes: read.bytes,
    })
}

fn show_repository_item(
    ctx: &PageContext,
    right: &Rc<right::RightPane>,
    pending_save: &PendingSaveState,
    file_monitor: &OpenedFileMonitorState,
    displayed_preview: &DisplayedPreviewState,
    item: RepositoryItem,
    selection: Option<(usize, usize)>,
) {
    let file_path = item.workspace_path.relative_or_empty();
    let displayed = DisplayedPreview {
        system_path: item.metadata.path.clone(),
        signature: PreviewSignature::Disk(provider::disk_signature(&item.metadata)),
    };
    if displayed_preview.borrow().as_ref() == Some(&displayed) {
        log::debug!("skip unchanged file preview file_path={file_path}");
        return;
    }

    if item.metadata.kind == FileKind::File {
        file_monitor.watch_file(
            ctx,
            right,
            pending_save,
            &item.workspace,
            &item.workspace_path,
            file_path,
            item.local_path.as_deref(),
            Some(provider::disk_signature(&item.metadata)),
        );
    } else {
        file_monitor.stop();
    }

    displayed_preview.replace(Some(displayed));
    let load_token = right.begin_preview_load(file_path);
    let selected_provider =
        provider::for_file(file_path, &item.metadata, item.prefetched_bytes.as_deref());
    match selection {
        Some((start, end)) => {
            (selected_provider.show_match)(provider::PreviewMatchRequest {
                ctx: ctx.clone(),
                right: Rc::clone(right),
                load_token,
                files: Arc::clone(&item.files),
                file_path,
                workspace_path: &item.workspace_path,
                local_path: item.local_path.as_deref(),
                metadata: &item.metadata,
                prefetched_bytes: item.prefetched_bytes.as_deref(),
                start,
                end,
            });
        }
        None => {
            (selected_provider.show)(provider::PreviewRequest {
                ctx: ctx.clone(),
                right: Rc::clone(right),
                load_token,
                files: Arc::clone(&item.files),
                file_path,
                workspace_path: &item.workspace_path,
                local_path: item.local_path.as_deref(),
                metadata: &item.metadata,
                prefetched_bytes: item.prefetched_bytes.as_deref(),
            });
        }
    }
}

fn add_gitignore_pattern(ctx: &PageContext, pattern: &str) {
    let Some(files) = ctx.files() else {
        ctx.show_error(
            "Ignore Failed",
            "File access is unavailable for this workspace.",
        );
        return;
    };
    match gitignore::add_pattern_to_workspace(files.as_ref(), &ctx.workspace_ref(), pattern) {
        Ok(message) => ctx.refresh(Some(message)),
        Err(err) => ctx.show_error("Ignore Failed", &err),
    }
}

fn run_container_file_action(ctx: &PageContext, file_path: &str, action: ContainerFileAction) {
    let Some(docker) = ctx.docker() else {
        ctx.show_error(
            "Container Action Failed",
            "Docker is unavailable for this workspace.",
        );
        return;
    };
    let workspace_path = ctx.workspace_ref().path(file_path);
    let (command, title, success_message) = match action {
        ContainerFileAction::BuildImage => match docker.build_image_command(&workspace_path) {
            Ok(command) => (
                command,
                "Docker Build",
                "Started Docker image build in terminal.",
            ),
            Err(err) => {
                ctx.show_error("Container Action Failed", &err);
                return;
            }
        },
        ContainerFileAction::ComposeUp => {
            match docker.compose_file_command(&workspace_path, ComposeFileAction::Up) {
                Ok(command) => (command, "Compose Up", "Started Compose Up in terminal."),
                Err(err) => {
                    ctx.show_error("Container Action Failed", &err);
                    return;
                }
            }
        }
        ContainerFileAction::ComposePull => {
            match docker.compose_file_command(&workspace_path, ComposeFileAction::Pull) {
                Ok(command) => (command, "Compose Pull", "Started Compose Pull in terminal."),
                Err(err) => {
                    ctx.show_error("Container Action Failed", &err);
                    return;
                }
            }
        }
        ContainerFileAction::ComposeRestart => {
            match docker.compose_file_command(&workspace_path, ComposeFileAction::Restart) {
                Ok(command) => (
                    command,
                    "Compose Restart",
                    "Started Compose Restart in terminal.",
                ),
                Err(err) => {
                    ctx.show_error("Container Action Failed", &err);
                    return;
                }
            }
        }
        ContainerFileAction::ComposeDown => {
            match docker.compose_file_command(&workspace_path, ComposeFileAction::Down) {
                Ok(command) => (command, "Compose Down", "Started Compose Down in terminal."),
                Err(err) => {
                    ctx.show_error("Container Action Failed", &err);
                    return;
                }
            }
        }
    };

    log::info!("repository container action start action={action:?} file_path={file_path}");
    match ctx.run_shell_command(&command, title) {
        Ok(()) => {
            log::info!(
                "repository container action terminal opened action={action:?} file_path={file_path}"
            );
            ctx.show_toast(success_message);
        }
        Err(err) => {
            log::warn!(
                "repository container action failed to open terminal action={action:?} file_path={file_path}: {err}",
            );
            ctx.show_error("Docker Action Failed", &err);
        }
    }
}

fn clear_displayed_preview_if_workspace_changed(
    displayed_preview: &DisplayedPreviewState,
    workspace: &WorkspaceRef,
) {
    let repo_changed = displayed_preview
        .borrow()
        .as_ref()
        .is_some_and(|displayed| displayed.system_path.workspace != *workspace);
    if repo_changed {
        displayed_preview.borrow_mut().take();
    }
}

fn flush_pending_save(
    ctx: &PageContext,
    right: &right::RightPane,
    pending_save: &PendingSaveState,
) -> bool {
    let Some(pending) = pending_save.borrow().clone() else {
        return true;
    };
    if right.file_editor_path.borrow().as_deref() != Some(pending.path.as_str()) {
        return true;
    }

    let current_signature = disk_signature_for_path(ctx, &pending.path).ok().flatten();
    if current_signature != pending.base_signature {
        pending_save.borrow_mut().take();
        return true;
    }

    let text = right.file_editor.document_text();
    if let Err(err) = write_repository_file(ctx, &pending.path, &text) {
        ctx.show_error("Save Failed", &err);
        return false;
    }
    if let Ok(signature) = disk_signature_for_path(ctx, &pending.path) {
        right.file_editor_disk_signature.replace(signature);
    }
    clear_pending_save(pending_save, &pending.path, pending.generation);
    true
}

fn pending_save_path(pending_save: &PendingSaveState) -> Option<String> {
    pending_save
        .borrow()
        .as_ref()
        .map(|pending| pending.path.clone())
}

fn pending_save_matches(pending_save: &PendingSaveState, path: &str, generation: u64) -> bool {
    pending_save
        .borrow()
        .as_ref()
        .is_some_and(|pending| pending.path == path && pending.generation == generation)
}

fn clear_pending_save(pending_save: &PendingSaveState, path: &str, generation: u64) {
    let should_clear = pending_save
        .borrow()
        .as_ref()
        .is_some_and(|pending| pending.path == path && pending.generation == generation);
    if should_clear {
        pending_save.borrow_mut().take();
    }
}

fn disk_signature_for_path(
    ctx: &PageContext,
    file_path: &str,
) -> Result<Option<provider::DiskSignature>, String> {
    let files = ctx
        .files()
        .ok_or_else(|| "File access is unavailable for this workspace.".to_string())?;
    let path = ctx.workspace_ref().path(file_path);
    let metadata = files.metadata(&path)?;
    if metadata.kind != FileKind::File {
        return Ok(None);
    }

    Ok(Some(provider::disk_signature(&metadata)))
}

pub(in crate::ui::pages::code) fn folder_entry_counts(
    files: &dyn FileAccess,
    path: &WorkspacePath,
) -> Result<(usize, usize), String> {
    let mut file_count = 0usize;
    let mut folder_count = 0usize;

    let entries = files
        .list_dirs(std::slice::from_ref(path))?
        .into_iter()
        .next()
        .map(|listing| listing.entries)
        .unwrap_or_default();
    for entry in entries {
        if entry.kind == FileKind::Directory {
            folder_count += 1;
        } else if entry.kind == FileKind::File {
            file_count += 1;
        }
    }

    Ok((file_count, folder_count))
}

pub(in crate::ui::pages::code) fn read_repository_file(
    files: &dyn FileAccess,
    path: &WorkspacePath,
) -> Result<String, String> {
    files
        .read_with_metadata(path, Some(MAX_EDITOR_FILE_BYTES))?
        .into_text()
}

pub(in crate::ui::pages::code) fn read_repository_file_bytes(
    files: &dyn FileAccess,
    path: &WorkspacePath,
) -> Result<Vec<u8>, String> {
    files
        .read_with_metadata(path, Some(MAX_EDITOR_FILE_BYTES))?
        .into_bytes()
}

pub(in crate::ui::pages::code) fn read_repository_file_from_prefetch(
    prefetched_bytes: Option<Vec<u8>>,
    files: &dyn FileAccess,
    path: &WorkspacePath,
) -> Result<String, String> {
    match prefetched_bytes {
        Some(bytes) => text_from_repository_bytes(bytes),
        None => read_repository_file(files, path),
    }
}

pub(in crate::ui::pages::code) fn read_repository_file_bytes_from_prefetch(
    prefetched_bytes: Option<Vec<u8>>,
    files: &dyn FileAccess,
    path: &WorkspacePath,
) -> Result<Vec<u8>, String> {
    match prefetched_bytes {
        Some(bytes) => Ok(bytes),
        None => read_repository_file_bytes(files, path),
    }
}

fn spellcheck_editor_document(
    ctx: &PageContext,
    file_editor: &code_editor::CodeEditor,
    file_path: &str,
    text: &str,
) {
    let Some(files) = ctx.files() else {
        file_editor.set_spellcheck_issues(Vec::new());
        return;
    };
    let allowlist = crate::spellcheck::load_manifest_allowlist(&ctx.workspace_ref(), files);
    let language = code_editor::language_hint_from_path(file_path);
    let issues = crate::spellcheck::check_document(&language, Some(file_path), text, &allowlist);
    file_editor.set_spellcheck_issues(issues);
}

fn text_from_repository_bytes(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err("Binary file preview is unavailable.".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8 text.".to_string())
}

fn write_repository_file(ctx: &PageContext, file_path: &str, text: &str) -> Result<(), String> {
    let files = ctx
        .files()
        .ok_or_else(|| "File access is unavailable for this workspace.".to_string())?;
    let path = ctx.workspace_ref().path(file_path);
    let metadata = files.metadata(&path)?;
    if metadata.kind != FileKind::File {
        return Err("Select a file to edit.".to_string());
    }

    files.write_text(&path, text)
}

fn local_path_for_system_path(system_path: &SystemPath) -> Option<PathBuf> {
    (system_path.system.provider_kind == ProviderKind::Local)
        .then(|| PathBuf::from(&system_path.path.absolute))
}

fn browser_terminal_dir(
    workspace_root: &WorkspacePath,
    file_path: &str,
    is_dir: bool,
) -> WorkspacePath {
    if is_dir {
        return WorkspacePath::from_workspace_relative(workspace_root, file_path);
    }
    let parent = file_path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    WorkspacePath::from_workspace_relative(workspace_root, parent)
}

fn run_repository_file_in_terminal(ctx: &PageContext, file_path: &str) -> Result<(), String> {
    let shell = ctx
        .shell()
        .ok_or_else(|| "Terminal is unavailable for this workspace.".to_string())?;
    let workspace = ctx.workspace_ref();
    let file = workspace.path(file_path);
    let working_dir = browser_terminal_dir(&workspace.root, file_path, false);
    let command = shell.command(&working_dir, &file.absolute, &[])?;
    let title = file
        .file_name()
        .map(|name| format!("Run {name}"))
        .unwrap_or_else(|| "Run File".to_string());
    log::info!(
        "repository executable terminal start file_path={} working_dir={}",
        file_path,
        working_dir.display()
    );
    ctx.run_shell_command(&command, &title)
}

fn file_watch_changes_include_path(
    changes: &FileWatchChanges,
    watched_path: &WorkspacePath,
) -> bool {
    changes.is_empty()
        || changes.iter().any(|changed_path| {
            changed_path == watched_path
                || changed_path.relative_or_empty() == watched_path.relative_or_empty()
        })
}
