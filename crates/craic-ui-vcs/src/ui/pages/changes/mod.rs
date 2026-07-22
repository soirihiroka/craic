mod left;
mod right;

use super::{
    Page, PageBadge, PageCommand, PageCommandResult, PageContext, PageInitializeComplete,
    PageRefreshComplete,
};
use crate::git::{
    self, BytesComparison, FileComparison, GitRepoHandle, OperationCallback, RepositorySnapshot,
    WorkspaceSnapshot,
};
use crate::github::CommitEmailOption;
use crate::gitignore::{self, IgnoreTargetKind};
use crate::system::capabilities::open::{DesktopOpenActivation, DesktopOpenTargetKind};
use crate::system::capabilities::url::UrlOpenActivation;
use crate::system::path::ProviderKind;
use crate::ui::components::context_menu;
use crate::ui::file_manager;
use crate::ui::file_type::PreviewKind;
use crate::ui::sidebar::changes_panel::ChangesPanel;
use crate::ui::sidebar::commit_panel::CommitPanel;
use crate::ui::widgets;
use adw::prelude::*;
use craic_ui_core::ui::command_mailbox;
use gtk::gio;
use gtk::gio::prelude::AppInfoExt;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use right::ChangesRight;

pub struct ChangesPage {
    ctx: PageContext,
    commit_form: CommitPanel,
    panel: Rc<ChangesPanel>,
    right: Rc<ChangesRight>,
    changed_count: Cell<usize>,
    active_preview_signature: Rc<RefCell<Option<WorktreePreviewSignature>>>,
    active_preview_subscription: Rc<RefCell<Option<ActivePreviewWatch>>>,
    preview_signatures: Rc<RefCell<HashMap<String, WorktreePreviewSignature>>>,
    preview_cache: Rc<RefCell<BoundedPreviewCache<WorktreePreviewSignature, WorktreePreview>>>,
    preview_workspace_key: Rc<RefCell<Option<String>>>,
    commit_message_generation_running: Rc<Cell<bool>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
}

const WORKTREE_PREVIEW_CACHE_LIMIT: usize = 24;
const COMMIT_EMAIL_ROW_DATA_KEY: &str = "commit-email-option";

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreePreviewSignature {
    path: String,
    kind: PreviewKind,
    status: Option<String>,
    head: Option<String>,
    disk: WorktreePreviewDiskSignature,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorktreePreviewDiskSignature {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
}

#[derive(Clone, Debug)]
enum WorktreePreview {
    Diff(FileComparison),
    Bytes(BytesComparison),
    PreviewLimit(String),
}

struct WorktreePreviewWorkerResult {
    signature: WorktreePreviewSignature,
    result: Result<WorktreePreview, String>,
    duration: Duration,
    cacheable: bool,
}

struct ActivePreviewWatch {
    _git: git::FileDiffSubscription,
    _updates: command_mailbox::UiCommandSubscription,
}

struct BoundedPreviewCache<K, V> {
    limit: usize,
    entries: VecDeque<(K, V)>,
}

impl<K: Eq + Clone, V: Clone> BoundedPreviewCache<K, V> {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let index = self.entries.iter().position(|(entry, _)| entry == key)?;
        let (key, value) = self.entries.remove(index)?;
        let cloned = value.clone();
        self.entries.push_front((key, value));
        Some(cloned)
    }

    fn insert(&mut self, key: K, value: V) -> usize {
        if let Some(index) = self.entries.iter().position(|(entry, _)| entry == &key) {
            self.entries.remove(index);
        }
        self.entries.push_front((key, value));

        let mut evicted = 0;
        while self.entries.len() > self.limit {
            self.entries.pop_back();
            evicted += 1;
        }
        evicted
    }

    fn retain<F>(&mut self, mut keep: F) -> usize
    where
        F: FnMut(&K) -> bool,
    {
        let before = self.entries.len();
        self.entries.retain(|(key, _)| keep(key));
        before.saturating_sub(self.entries.len())
    }

    fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }
}

impl ChangesPage {
    pub fn new(ctx: PageContext) -> Self {
        let (commit_form, panel) = left::build();
        let page = Self {
            ctx,
            commit_form,
            panel: Rc::new(panel),
            right: Rc::new(ChangesRight::new()),
            changed_count: Cell::new(0),
            active_preview_signature: Rc::new(RefCell::new(None)),
            active_preview_subscription: Rc::new(RefCell::new(None)),
            preview_signatures: Rc::new(RefCell::new(HashMap::new())),
            preview_cache: Rc::new(RefCell::new(BoundedPreviewCache::new(
                WORKTREE_PREVIEW_CACHE_LIMIT,
            ))),
            preview_workspace_key: Rc::new(RefCell::new(None)),
            commit_message_generation_running: Rc::new(Cell::new(false)),
            active_context_menu: Rc::new(RefCell::new(None)),
        };
        page.connect_file_selection();
        page.connect_commit_actions();
        page.connect_repository_suggestions();
        page.connect_repository_initialization();
        page.connect_context_menus();
        page
    }

    fn connect_file_selection(&self) {
        self.panel.files_list.connect_row_selected({
            let ctx = self.ctx.clone();
            let right = self.right.clone();
            let active_preview_signature = self.active_preview_signature.clone();
            let active_preview_subscription = self.active_preview_subscription.clone();
            let preview_signatures = self.preview_signatures.clone();
            let preview_cache = self.preview_cache.clone();

            move |_, row| {
                let Some(file_path) = row
                    .map(|row| row.widget_name().to_string())
                    .filter(|path| !path.is_empty())
                else {
                    active_preview_signature.borrow_mut().take();
                    active_preview_subscription.borrow_mut().take();
                    log::info!(
                        "changes preview selection cleared workspace={}",
                        ctx.workspace_key()
                    );
                    right.show_home();
                    return;
                };
                let Some(signature) = preview_signatures.borrow().get(&file_path).cloned() else {
                    active_preview_signature.borrow_mut().take();
                    active_preview_subscription.borrow_mut().take();
                    log::warn!(
                        "changes preview signature missing workspace={} path={}",
                        ctx.workspace_key(),
                        file_path
                    );
                    right.show_home();
                    return;
                };
                if active_preview_signature.borrow().as_ref() == Some(&signature) {
                    log::debug!(
                        "changes preview selection unchanged workspace={} path={} kind={:?}",
                        ctx.workspace_key(),
                        signature.path,
                        signature.kind
                    );
                    return;
                }
                show_worktree_preview(
                    &ctx,
                    &right,
                    &active_preview_signature,
                    &active_preview_subscription,
                    &preview_cache,
                    signature,
                );
            }
        });
    }

    fn connect_commit_actions(&self) {
        self.commit_form.commit_button.connect_clicked({
            let ctx = self.ctx.clone();
            let panel = self.panel.clone();
            let summary_entry = self.commit_form.summary_entry.clone();
            let description_view = self.commit_form.description_view.clone();

            move |_| {
                let summary = panel.commit_summary();
                let description = left::text_view_text(&description_view);
                let files = panel.checked_file_paths();
                let Some(git_handle) = ctx.git() else {
                    ctx.show_error("Commit Failed", &ctx.git_unavailable_message());
                    return;
                };
                let completion = command_mailbox::once({
                    let ctx = ctx.clone();
                    let summary_entry = summary_entry.clone();
                    let description_view = description_view.clone();

                    move |result: Result<String, String>| match result {
                        Ok(output) => {
                            summary_entry.set_text("");
                            description_view.buffer().set_text("");
                            let message = if output.is_empty() {
                                "Commit created.".to_string()
                            } else {
                                output
                            };
                            ctx.refresh_without_toast(Some(message));
                        }
                        Err(err) => ctx.show_error("Commit Failed", &err),
                    }
                });
                git_handle.commit_paths(
                    &summary,
                    &description,
                    &files,
                    Box::new(move |result| {
                        completion.send(result);
                    }),
                );
            }
        });

        self.commit_form.avatar_button.connect_clicked({
            let ctx = self.ctx.clone();
            let avatar_button = self.commit_form.avatar_button.clone();
            let active_context_menu = self.active_context_menu.clone();

            move |_| {
                show_commit_author_email_selector(&ctx, &avatar_button, &active_context_menu);
            }
        });

        self.commit_form.generate_button.connect_clicked({
            let ctx = self.ctx.clone();
            let panel = self.panel.clone();
            let summary_entry = self.commit_form.summary_entry.clone();
            let description_view = self.commit_form.description_view.clone();
            let generate_button = self.commit_form.generate_button.clone();
            let generate_icon_stack = self.commit_form.generate_icon_stack.clone();
            let running = self.commit_message_generation_running.clone();
            let hovered = Rc::new(Cell::new(false));
            let active_cancel = Rc::new(RefCell::new(
                None::<crate::agent_provider::CancellationToken>,
            ));
            let generation_request_id = Rc::new(Cell::new(0u64));

            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter({
                let generate_button = generate_button.clone();
                let generate_icon_stack = generate_icon_stack.clone();
                let running = running.clone();
                let hovered = hovered.clone();

                move |_, _, _| {
                    hovered.set(true);
                    if running.get() {
                        set_commit_message_generation_button_running(
                            &generate_button,
                            &generate_icon_stack,
                            true,
                        );
                    }
                }
            });
            motion.connect_leave({
                let generate_button = generate_button.clone();
                let generate_icon_stack = generate_icon_stack.clone();
                let running = running.clone();
                let hovered = hovered.clone();

                move |_| {
                    hovered.set(false);
                    if running.get() {
                        set_commit_message_generation_button_running(
                            &generate_button,
                            &generate_icon_stack,
                            false,
                        );
                    }
                }
            });
            generate_button.add_controller(motion);

            move |_| {
                if running.get() {
                    cancel_commit_message_generation(
                        &panel,
                        &summary_entry,
                        &description_view,
                        &generate_button,
                        &generate_icon_stack,
                        running.clone(),
                        &active_cancel,
                        &generation_request_id,
                    );
                    return;
                }

                generate_commit_message(
                    &ctx,
                    &panel,
                    &summary_entry,
                    &description_view,
                    &generate_button,
                    &generate_icon_stack,
                    running.clone(),
                    &active_cancel,
                    &generation_request_id,
                    hovered.clone(),
                );
            }
        });
    }

    fn connect_repository_suggestions(&self) {
        let actions = &self.right.suggestions_actions;
        let local_workspace = self.ctx.local_workspace_path().is_some();
        let desktop_open_available = local_workspace && self.ctx.desktop_opener().is_some();
        actions.open_editor.set_sensitive(local_workspace);
        actions.open_terminal.set_sensitive(local_workspace);
        actions.show_files.set_sensitive(desktop_open_available);
        let terminal_event_time = track_button_event_time(&actions.open_terminal);

        // Repository suggestions are app-launch affordances. Do not route them
        // to integrated Craic UI: Ghostty must open external Ghostty, and Files
        // must open the external desktop file manager with Wayland activation.
        actions.open_editor.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| launch_repo_command(&ctx, "code", &[], "Opened in editor.")
        });

        actions.open_terminal.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| open_repository_in_ghostty(&ctx, terminal_event_time.get())
        });

        actions.show_files.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| open_repository_in_files(&ctx)
        });

        actions.view_github.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| open_remote_repository(&ctx)
        });

        actions.git_button.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| ctx.run_git_action()
        });
    }

    fn connect_repository_initialization(&self) {
        self.right.initialize_button.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| initialize_git_repository(&ctx)
        });
        self.panel.initialize_button.connect_clicked({
            let ctx = self.ctx.clone();
            move |_| initialize_git_repository(&ctx)
        });
    }

    fn connect_context_menus(&self) {
        let files_list = self.panel.files_list.clone();
        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let ctx = self.ctx.clone();
            let active_context_menu = self.active_context_menu.clone();

            move |gesture, _, x, y| {
                if gesture.current_button() != 3 {
                    return;
                }

                let Some(row) = files_list.row_at_y(y as i32) else {
                    return;
                };
                let file_path = row.widget_name().to_string();
                if file_path.is_empty() {
                    return;
                }

                let parent = files_list.clone();
                let ctx = ctx.clone();
                let active_context_menu = active_context_menu.clone();
                gtk::glib::idle_add_local_once(move || {
                    show_changed_file_context_menu(
                        &ctx,
                        &parent,
                        &active_context_menu,
                        &file_path,
                        x,
                        y,
                    );
                });
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.panel.files_list.add_controller(click);

        let selection_header = self.panel.selection_header.clone();
        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let ctx = self.ctx.clone();
            let panel = self.panel.clone();
            let active_context_menu = self.active_context_menu.clone();
            let parent = selection_header.clone();

            move |gesture, _, x, y| {
                if gesture.current_button() != 3 {
                    return;
                }

                let ctx = ctx.clone();
                let panel = panel.clone();
                let active_context_menu = active_context_menu.clone();
                let parent = parent.clone();
                gtk::glib::idle_add_local_once(move || {
                    show_changed_file_selector_context_menu(
                        &ctx,
                        &panel,
                        &parent,
                        &active_context_menu,
                        x,
                        y,
                    );
                });
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.panel.selection_header.add_controller(click);
    }
}

impl Page for ChangesPage {
    fn label(&self) -> &'static str {
        "Changes"
    }

    fn icon_name(&self) -> &'static str {
        "document-edit-symbolic"
    }

    fn initialize(&self, completion: PageInitializeComplete) {
        completion(
            self.panel.root.clone().upcast(),
            self.right.root.clone().upcast(),
        );
    }

    fn refresh(&self, snapshot: &WorkspaceSnapshot, completion: PageRefreshComplete) {
        let Some(snapshot) = snapshot.repository() else {
            self.set_error("Not a git repository.");
            completion();
            return;
        };

        sync_worktree_preview_workspace(
            &self.preview_workspace_key,
            &self.preview_signatures,
            &self.preview_cache,
            &self.active_preview_signature,
            &self.active_preview_subscription,
            &self.ctx.workspace_key(),
        );

        let changed_count = snapshot.changed_files.len();
        if self.changed_count.replace(changed_count) != changed_count {
            self.ctx.notify_badge_changed();
        }
        self.commit_form.set_branch(&snapshot.branch);
        self.commit_form.update_avatar(snapshot);
        replace_worktree_preview_signatures(&self.preview_signatures, snapshot);
        self.panel.update(snapshot);
        self.right.update(snapshot, self.ctx.git_action_running());
        retain_worktree_preview_cache(&self.preview_cache, snapshot, &self.ctx.workspace_key());

        if let Some(file_path) = self.panel.selected_file_path() {
            let signature = worktree_preview_signature(snapshot, &file_path);
            if self.active_preview_signature.borrow().as_ref() == Some(&signature) {
                completion();
                return;
            }
            show_worktree_preview(
                &self.ctx,
                &self.right,
                &self.active_preview_signature,
                &self.active_preview_subscription,
                &self.preview_cache,
                signature,
            );
        } else {
            self.active_preview_signature.borrow_mut().take();
            self.active_preview_subscription.borrow_mut().take();
            self.right.show_home();
        }
        completion();
    }

    fn set_error(&self, message: &str) {
        if self.changed_count.replace(0) != 0 {
            self.ctx.notify_badge_changed();
        }
        self.commit_form.clear();
        self.panel.clear();
        self.active_preview_signature.borrow_mut().take();
        self.active_preview_subscription.borrow_mut().take();
        self.preview_signatures.borrow_mut().clear();
        clear_worktree_preview_cache(
            &self.preview_cache,
            "repository-error",
            &self.ctx.workspace_key(),
        );
        if error_can_initialize_git(&self.ctx, message) {
            log::info!(
                "changes page showing git initialization status workspace={}",
                self.ctx.workspace_key()
            );
            self.panel.show_initialize_repository();
            self.right.show_initialize_repository();
        } else {
            self.right.set_error(message);
        }
    }

    fn badge(&self) -> Option<PageBadge> {
        let count = self.changed_count.get();
        (count > 0).then(|| PageBadge::new(count.to_string()))
    }

    fn toggle_left_search(&self) -> bool {
        self.panel.toggle_search();
        true
    }

    fn toggle_right_search(&self) -> bool {
        self.right.toggle_search()
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        match command {
            PageCommand::ClearSelection => {
                self.panel.files_list.unselect_all();
                self.active_preview_signature.borrow_mut().take();
                self.active_preview_subscription.borrow_mut().take();
                log::info!(
                    "changes preview selection cleared workspace={}",
                    self.ctx.workspace_key()
                );
                self.right.show_home();
                PageCommandResult::Handled
            }
            _ => PageCommandResult::Ignored,
        }
    }
}

fn error_can_initialize_git(ctx: &PageContext, message: &str) -> bool {
    ctx.git().is_some() && is_not_git_repository_error(message)
}

fn is_not_git_repository_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("not a git repository")
        || message.contains("not in a git directory")
        || message.contains("outside repository")
}

fn sync_worktree_preview_workspace(
    preview_workspace_key: &Rc<RefCell<Option<String>>>,
    preview_signatures: &Rc<RefCell<HashMap<String, WorktreePreviewSignature>>>,
    preview_cache: &Rc<RefCell<BoundedPreviewCache<WorktreePreviewSignature, WorktreePreview>>>,
    active_preview_signature: &Rc<RefCell<Option<WorktreePreviewSignature>>>,
    active_preview_subscription: &Rc<RefCell<Option<ActivePreviewWatch>>>,
    workspace_key: &str,
) {
    if preview_workspace_key.borrow().as_deref() == Some(workspace_key) {
        return;
    }

    let previous = preview_workspace_key.replace(Some(workspace_key.to_string()));
    active_preview_signature.borrow_mut().take();
    active_preview_subscription.borrow_mut().take();
    preview_signatures.borrow_mut().clear();
    let invalidated = preview_cache.borrow_mut().clear();
    log::info!(
        "changes preview cache invalidation workspace={} previous_workspace={:?} reason=workspace-change count={}",
        workspace_key,
        previous,
        invalidated
    );
}

fn replace_worktree_preview_signatures(
    preview_signatures: &Rc<RefCell<HashMap<String, WorktreePreviewSignature>>>,
    snapshot: &RepositorySnapshot,
) {
    let mut signatures = HashMap::new();
    for file in &snapshot.changed_files {
        signatures.insert(
            file.path.clone(),
            worktree_preview_signature(snapshot, &file.path),
        );
    }
    preview_signatures.replace(signatures);
}

fn retain_worktree_preview_cache(
    preview_cache: &Rc<RefCell<BoundedPreviewCache<WorktreePreviewSignature, WorktreePreview>>>,
    snapshot: &RepositorySnapshot,
    workspace_key: &str,
) {
    let invalidated = preview_cache.borrow_mut().retain(|signature| {
        snapshot
            .changed_files
            .iter()
            .any(|file| file.path == signature.path)
            && worktree_preview_signature(snapshot, &signature.path) == *signature
    });
    if invalidated > 0 {
        log::info!(
            "changes preview cache invalidation workspace={} reason=signature-change count={}",
            workspace_key,
            invalidated
        );
    }
}

fn clear_worktree_preview_cache(
    preview_cache: &Rc<RefCell<BoundedPreviewCache<WorktreePreviewSignature, WorktreePreview>>>,
    reason: &str,
    workspace_key: &str,
) {
    let invalidated = preview_cache.borrow_mut().clear();
    if invalidated > 0 {
        log::info!(
            "changes preview cache invalidation workspace={} reason={} count={}",
            workspace_key,
            reason,
            invalidated
        );
    }
}

fn initialize_git_repository(ctx: &PageContext) {
    let Some(git_handle) = ctx.git() else {
        ctx.show_error(
            "Initialize Repository Failed",
            "Git repository initialization is unavailable for this workspace.",
        );
        return;
    };

    let ctx = ctx.clone();
    let completion = command_mailbox::once(move |result: Result<String, String>| match result {
        Ok(message) => ctx.refresh(Some(message)),
        Err(err) => ctx.show_error("Initialize Repository Failed", &err),
    });
    git_handle.initialize_repository(Box::new(move |result| {
        completion.send(result);
    }));
}

fn show_worktree_preview(
    ctx: &PageContext,
    right: &Rc<ChangesRight>,
    active_preview_signature: &Rc<RefCell<Option<WorktreePreviewSignature>>>,
    active_preview_subscription: &Rc<RefCell<Option<ActivePreviewWatch>>>,
    preview_cache: &Rc<RefCell<BoundedPreviewCache<WorktreePreviewSignature, WorktreePreview>>>,
    signature: WorktreePreviewSignature,
) {
    let file_path = signature.path.clone();
    active_preview_signature.replace(Some(signature.clone()));
    active_preview_subscription.borrow_mut().take();

    if let Some(preview) = preview_cache.borrow_mut().get(&signature) {
        log::info!(
            "changes preview cache hit workspace={} path={} kind={:?} {}",
            ctx.workspace_key(),
            file_path,
            signature.kind,
            worktree_preview_summary(&preview)
        );
        show_worktree_preview_outcome(right, &file_path, &preview);
    } else {
        log::info!(
            "changes preview cache miss workspace={} path={} kind={:?}",
            ctx.workspace_key(),
            file_path,
            signature.kind
        );
        right.show_loading(&file_path);
    }

    let Some(git_handle) = ctx.git() else {
        active_preview_signature.borrow_mut().take();
        right.show_home();
        ctx.show_error("Diff Failed", &ctx.git_unavailable_message());
        return;
    };
    let workspace_key = ctx.workspace_key();
    let (sender, updates) = command_mailbox::latest({
        let ctx = ctx.clone();
        let right = right.clone();
        let active_preview_signature = active_preview_signature.clone();
        let active_preview_subscription = active_preview_subscription.clone();
        let preview_cache = preview_cache.clone();

        move |result: WorktreePreviewWorkerResult| {
            let is_current = ctx.workspace_is_current(&workspace_key)
                && active_preview_signature.borrow().as_ref() == Some(&result.signature);
            if !is_current {
                log::info!(
                    "changes preview stale result dropped workspace={} path={} kind={:?} duration_ms={}",
                    workspace_key,
                    result.signature.path,
                    result.signature.kind,
                    result.duration.as_millis()
                );
                return;
            }

            match result.result {
                Ok(preview) => {
                    log::info!(
                        "changes preview loaded workspace={} path={} kind={:?} duration_ms={} {}",
                        workspace_key,
                        result.signature.path,
                        result.signature.kind,
                        result.duration.as_millis(),
                        worktree_preview_summary(&preview)
                    );
                    if result.cacheable {
                        let evicted = preview_cache
                            .borrow_mut()
                            .insert(result.signature.clone(), preview.clone());
                        if evicted > 0 {
                            log::info!(
                                "changes preview cache invalidation workspace={} reason=evict count={}",
                                workspace_key,
                                evicted
                            );
                        }
                    }
                    show_worktree_preview_outcome(&right, &result.signature.path, &preview);
                }
                Err(err) => {
                    active_preview_signature.borrow_mut().take();
                    right.show_home();
                    log::warn!(
                        "changes preview load failed workspace={} path={} kind={:?} duration_ms={} err={}",
                        workspace_key,
                        result.signature.path,
                        result.signature.kind,
                        result.duration.as_millis(),
                        err
                    );
                    ctx.show_error("Diff Failed", &err);
                    let active_preview_subscription = active_preview_subscription.clone();
                    gtk::glib::idle_add_local_once(move || {
                        active_preview_subscription.borrow_mut().take();
                    });
                }
            }
        }
    });

    let git = start_worktree_preview_watch(git_handle, signature, sender);
    active_preview_subscription.replace(Some(ActivePreviewWatch {
        _git: git,
        _updates: updates,
    }));
}

fn start_worktree_preview_watch(
    git_handle: Arc<GitRepoHandle>,
    signature: WorktreePreviewSignature,
    sender: command_mailbox::UiCommandSender<WorktreePreviewWorkerResult>,
) -> git::FileDiffSubscription {
    let mut start = Instant::now();
    let mut cacheable = true;
    let path = signature.path.clone();
    match signature.kind {
        PreviewKind::Image
        | PreviewKind::Audio
        | PreviewKind::Video
        | PreviewKind::Font
        | PreviewKind::Pdf => git_handle.watch_bytes_comparison(
            &path,
            Box::new(move |result| {
                let result = match result.map(WorktreePreview::Bytes) {
                    Ok(preview) => Ok(preview),
                    Err(err) if is_preview_limit_message(&err) => {
                        Ok(WorktreePreview::PreviewLimit(err))
                    }
                    Err(err) => Err(err),
                };
                sender.send(WorktreePreviewWorkerResult {
                    signature: signature.clone(),
                    result,
                    duration: start.elapsed(),
                    cacheable,
                });
                start = Instant::now();
                cacheable = false;
            }),
        ),
        _ => git_handle.watch_comparison(
            &path,
            Box::new(move |result| {
                let result = match result.map(WorktreePreview::Diff) {
                    Ok(preview) => Ok(preview),
                    Err(err) if is_preview_limit_message(&err) => {
                        Ok(WorktreePreview::PreviewLimit(err))
                    }
                    Err(err) => Err(err),
                };
                sender.send(WorktreePreviewWorkerResult {
                    signature: signature.clone(),
                    result,
                    duration: start.elapsed(),
                    cacheable,
                });
                start = Instant::now();
                cacheable = false;
            }),
        ),
    }
}

fn show_worktree_preview_outcome(right: &ChangesRight, file_path: &str, preview: &WorktreePreview) {
    match preview {
        WorktreePreview::Diff(comparison) => right.show_comparison(file_path, comparison),
        WorktreePreview::Bytes(comparison) => right.show_binary_comparison(file_path, comparison),
        WorktreePreview::PreviewLimit(message) => {
            right.show_preview_unavailable(file_path, message);
        }
    }
}

fn worktree_preview_summary(preview: &WorktreePreview) -> String {
    match preview {
        WorktreePreview::Diff(comparison) => format!("rows={}", comparison.rows.len()),
        WorktreePreview::Bytes(comparison) => format!(
            "before_bytes={} after_bytes={}",
            comparison.before.as_ref().map(Vec::len).unwrap_or(0),
            comparison.after.as_ref().map(Vec::len).unwrap_or(0)
        ),
        WorktreePreview::PreviewLimit(_) => "preview_limit=true".to_string(),
    }
}

fn worktree_preview_signature(
    snapshot: &RepositorySnapshot,
    file_path: &str,
) -> WorktreePreviewSignature {
    let changed_file = snapshot
        .changed_files
        .iter()
        .find(|file| file.path == file_path);
    let signature = changed_file.and_then(|file| file.worktree_signature.as_ref());
    WorktreePreviewSignature {
        path: file_path.to_string(),
        kind: crate::ui::file_type::preview_kind_for_path(
            file_path,
            signature.is_some_and(|signature| signature.is_dir),
        ),
        status: snapshot
            .changed_files
            .iter()
            .find(|file| file.path == file_path)
            .map(|file| file.status.clone()),
        head: snapshot.history_head.clone(),
        disk: signature
            .filter(|signature| !signature.is_dir)
            .map(|signature| WorktreePreviewDiskSignature::Present {
                len: signature.len,
                modified: signature.modified,
            })
            .unwrap_or(WorktreePreviewDiskSignature::Missing),
    }
}

fn is_preview_limit_message(message: &str) -> bool {
    message.contains("too large to preview") || message.contains("cannot be previewed as text")
}

fn generate_commit_message(
    ctx: &PageContext,
    panel: &ChangesPanel,
    summary_entry: &gtk::Entry,
    description_view: &gtk::TextView,
    generate_button: &gtk::Button,
    generate_icon_stack: &gtk::Stack,
    running: Rc<Cell<bool>>,
    active_cancel: &Rc<RefCell<Option<crate::agent_provider::CancellationToken>>>,
    generation_request_id: &Rc<Cell<u64>>,
    hovered: Rc<Cell<bool>>,
) {
    if running.get() {
        return;
    }

    let files = panel.checked_file_paths();
    if files.is_empty() {
        ctx.show_error(
            "Generate Commit Message Failed",
            "Select at least one file before generating a commit message.",
        );
        return;
    }

    let Some(git_handle) = ctx.git() else {
        ctx.show_error(
            "Generate Commit Message Failed",
            "Commit message generation is unavailable for this workspace.",
        );
        return;
    };
    let app_config = crate::config::load();
    let provider_id = app_config.commit_message_provider;
    let provider_label = crate::agent_provider::find_provider(&provider_id)
        .map(|provider| provider.label().to_string())
        .unwrap_or_else(|| provider_id.clone());
    let model = app_config.commit_message_model;
    let cancellation = crate::agent_provider::CancellationToken::new();
    let request_id = generation_request_id.get().wrapping_add(1);
    generation_request_id.set(request_id);
    active_cancel.replace(Some(cancellation.clone()));

    running.set(true);
    summary_entry.set_sensitive(false);
    description_view.set_sensitive(false);
    set_commit_message_generation_button_running(
        generate_button,
        generate_icon_stack,
        hovered.get(),
    );

    let completion = command_mailbox::once({
        let ctx = ctx.clone();
        let panel = panel.clone();
        let summary_entry = summary_entry.clone();
        let description_view = description_view.clone();
        let generate_button = generate_button.clone();
        let generate_icon_stack = generate_icon_stack.clone();
        let active_cancel = active_cancel.clone();
        let generation_request_id = generation_request_id.clone();

        move |result: Result<crate::ai_commit::CommitMessageDraft, String>| {
            if generation_request_id.get() != request_id {
                return;
            }
            finish_commit_message_generation(
                &panel,
                &summary_entry,
                &description_view,
                &generate_button,
                &generate_icon_stack,
                running.clone(),
                &active_cancel,
            );
            match result {
                Ok(draft) => {
                    summary_entry.set_text(&draft.summary);
                    description_view.buffer().set_text(&draft.description);
                    ctx.refresh_without_toast(Some(format!(
                        "Generated commit message with {}.",
                        provider_label
                    )));
                }
                Err(err) if crate::agent_provider::is_canceled_error(&err) => {
                    log::info!("commit message generation canceled");
                }
                Err(err) => ctx.show_error("Generate Commit Message Failed", &err),
            }
        }
    });

    git_handle.commit_message_context(
        &files,
        Box::new(move |context_result| {
            let completion = completion.clone();
            let provider_id = provider_id.clone();
            let model = model.clone();
            let cancellation = cancellation.clone();
            thread::spawn(move || {
                let result = context_result.and_then(|context| {
                    crate::ai_commit::generate_from_context(
                        context,
                        &provider_id,
                        model.as_deref(),
                        &cancellation,
                    )
                });
                completion.send(result);
            });
        }),
    );
}

fn cancel_commit_message_generation(
    panel: &ChangesPanel,
    summary_entry: &gtk::Entry,
    description_view: &gtk::TextView,
    generate_button: &gtk::Button,
    generate_icon_stack: &gtk::Stack,
    running: Rc<Cell<bool>>,
    active_cancel: &Rc<RefCell<Option<crate::agent_provider::CancellationToken>>>,
    generation_request_id: &Rc<Cell<u64>>,
) {
    if let Some(cancellation) = active_cancel.borrow().as_ref() {
        cancellation.cancel();
    }
    generation_request_id.set(generation_request_id.get().wrapping_add(1));
    finish_commit_message_generation(
        panel,
        summary_entry,
        description_view,
        generate_button,
        generate_icon_stack,
        running,
        active_cancel,
    );
    log::info!("commit message generation cancel requested");
}

fn finish_commit_message_generation(
    panel: &ChangesPanel,
    summary_entry: &gtk::Entry,
    description_view: &gtk::TextView,
    generate_button: &gtk::Button,
    generate_icon_stack: &gtk::Stack,
    running: Rc<Cell<bool>>,
    active_cancel: &Rc<RefCell<Option<crate::agent_provider::CancellationToken>>>,
) {
    running.set(false);
    active_cancel.borrow_mut().take();
    summary_entry.set_sensitive(true);
    description_view.set_sensitive(true);
    generate_button.set_sensitive(!panel.checked_file_paths().is_empty());
    generate_button.set_tooltip_text(Some("Generate commit message"));
    generate_icon_stack.set_visible_child_name("icon");
}

fn set_commit_message_generation_button_running(
    generate_button: &gtk::Button,
    generate_icon_stack: &gtk::Stack,
    hovered: bool,
) {
    generate_button.set_sensitive(true);
    if hovered {
        generate_button.set_tooltip_text(Some("Cancel commit message generation"));
        generate_icon_stack.set_visible_child_name("cancel");
    } else {
        generate_button.set_tooltip_text(Some("Generating commit message"));
        generate_icon_stack.set_visible_child_name("spinner");
    }
}

fn show_commit_author_email_selector(
    ctx: &PageContext,
    parent: &gtk::Button,
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");
    let cached_emails = crate::github::cached_commit_email_options();
    if let Some(emails) = cached_emails.as_ref() {
        replace_commit_email_rows_with_loading(&list, emails, true);
    } else {
        list.append(&commit_email_loading_row());
    }

    let popover = gtk::Popover::builder()
        .width_request(280)
        .child(&list)
        .build();
    popover.add_css_class("menu");
    popover.set_has_arrow(true);
    popover.set_parent(parent);
    popover.set_position(gtk::PositionType::Bottom);

    list.connect_row_activated({
        let ctx = ctx.clone();
        let popover = popover.clone();

        move |_, row| {
            let Some(option) = commit_email_option_from_row(row) else {
                return;
            };

            let Some(git_handle) = ctx.git() else {
                ctx.show_error("Author Selection Failed", &ctx.git_unavailable_message());
                return;
            };

            let completion = command_mailbox::once({
                let ctx = ctx.clone();
                let popover = popover.clone();

                move |result: Result<(), String>| match result {
                    Ok(()) => {
                        log::info!("commit author updated from selector");
                        popover.popdown();
                        ctx.refresh(Some("Commit author updated.".to_string()));
                    }
                    Err(err) => ctx.show_error("Author Selection Failed", &err),
                }
            });
            git_handle.save_author_identity(
                &option.name,
                &option.email,
                Box::new(move |result| {
                    completion.send(result);
                }),
            );
        }
    });

    context_menu::retain_context_menu(active_context_menu, popover.upcast_ref::<gtk::Popover>());
    popover.popup();

    let completion = command_mailbox::once({
        let list = list.clone();
        let cached_emails = cached_emails.clone();

        move |result: Result<Vec<CommitEmailOption>, String>| match result {
            Ok(emails) => replace_commit_email_rows(&list, emails),
            Err(err) => {
                log::warn!("failed to load commit email selector options: {err}");
                if let Some(emails) = cached_emails.as_ref() {
                    replace_commit_email_rows_with_status(
                        &list,
                        emails,
                        "Could not refresh GitHub emails",
                        &err,
                    );
                } else {
                    replace_commit_email_error_row(&list, &err);
                }
            }
        }
    });
    thread::spawn(move || {
        let result = crate::github::commit_email_options();
        completion.send(result);
    });
}

fn commit_email_loading_row() -> gtk::ListBoxRow {
    let spinner = adw::Spinner::new();
    spinner.set_size_request(14, 14);
    let label = gtk::Label::builder()
        .label("Loading GitHub emails")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(5)
        .margin_bottom(5)
        .margin_start(8)
        .margin_end(8)
        .build();
    content.append(&spinner);
    content.append(&label);

    gtk::ListBoxRow::builder()
        .child(&content)
        .selectable(false)
        .activatable(false)
        .build()
}

fn replace_commit_email_rows(list: &gtk::ListBox, options: Vec<CommitEmailOption>) {
    replace_commit_email_rows_with_loading(list, &options, false);
}

fn replace_commit_email_rows_with_loading(
    list: &gtk::ListBox,
    options: &[CommitEmailOption],
    loading: bool,
) {
    clear_list_box(list);
    if options.is_empty() {
        list.append(&commit_email_status_row(
            "No GitHub emails found",
            "gh returned no commit emails",
        ));
        return;
    }

    for option in options {
        list.append(&commit_email_row(option));
    }

    if loading {
        list.append(&commit_email_loading_row());
    }
}

fn replace_commit_email_rows_with_status(
    list: &gtk::ListBox,
    options: &[CommitEmailOption],
    title: &str,
    subtitle: &str,
) {
    replace_commit_email_rows_with_loading(list, options, false);
    let row = commit_email_status_row(title, subtitle);
    row.set_tooltip_text(Some(subtitle));
    list.append(&row);
}

fn replace_commit_email_error_row(list: &gtk::ListBox, error: &str) {
    clear_list_box(list);
    let row = commit_email_status_row("Could not load GitHub emails", error);
    row.set_tooltip_text(Some(error));
    list.append(&row);
}

fn commit_email_row(option: &CommitEmailOption) -> gtk::ListBoxRow {
    let display_name = if option.name.is_empty() {
        option.email.as_str()
    } else {
        option.name.as_str()
    };

    let avatar = adw::Avatar::builder()
        .size(28)
        .text(display_name)
        .show_initials(true)
        .build();
    if let Some(url) = option.avatar_url.as_ref() {
        widgets::fetch_avatar(&avatar, widgets::AvatarSource::Url(url.clone()));
    }

    let title = gtk::Label::builder()
        .label(display_name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    let subtitle = gtk::Label::builder()
        .label(&option.email)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    subtitle.add_css_class("dim-label");
    subtitle.add_css_class("caption");
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    labels.append(&title);
    labels.append(&subtitle);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(3)
        .margin_bottom(3)
        .margin_start(4)
        .margin_end(4)
        .build();
    content.append(&avatar);
    content.append(&labels);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(&option.email);
    unsafe {
        row.set_data(COMMIT_EMAIL_ROW_DATA_KEY, option.clone());
    }
    row
}

fn commit_email_option_from_row(row: &gtk::ListBoxRow) -> Option<CommitEmailOption> {
    let option = unsafe { row.data::<CommitEmailOption>(COMMIT_EMAIL_ROW_DATA_KEY) }?;
    Some(unsafe { option.as_ref().clone() })
}

fn commit_email_status_row(title: &str, subtitle: &str) -> gtk::ListBoxRow {
    let title = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .build();
    let subtitle = gtk::Label::builder()
        .label(subtitle)
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    subtitle.add_css_class("dim-label");
    subtitle.add_css_class("caption");
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .margin_top(5)
        .margin_bottom(5)
        .margin_start(8)
        .margin_end(8)
        .build();
    content.append(&title);
    content.append(&subtitle);

    gtk::ListBoxRow::builder()
        .child(&content)
        .selectable(false)
        .activatable(false)
        .build()
}

fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn show_changed_file_selector_context_menu(
    ctx: &PageContext,
    panel: &ChangesPanel,
    parent: &gtk::Box,
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
    x: f64,
    y: f64,
) {
    let menu = changed_file_selector_menu();
    let has_changed_files = panel.has_changed_files();
    let actions = gio::SimpleActionGroup::new();

    let select_all = add_menu_action(&actions, "select-all", {
        let panel = panel.clone();
        move |_, _| panel.set_all_checked(true)
    });
    select_all.set_enabled(has_changed_files);

    let deselect_all = add_menu_action(&actions, "deselect-all", {
        let panel = panel.clone();
        move |_, _| panel.set_all_checked(false)
    });
    deselect_all.set_enabled(has_changed_files);

    let stash_all = add_menu_action(&actions, "stash-all", {
        let ctx = ctx.clone();
        move |_, _| {
            let Some(git_handle) = ctx.git() else {
                ctx.show_error("Stash Failed", &ctx.git_unavailable_message());
                return;
            };
            let completion = changes_git_string_command(
                &ctx,
                "Stash Failed",
                Some("Changes stashed.".to_string()),
            );
            git_handle.stash_changes(Box::new(move |result| {
                completion.send(result);
            }));
        }
    });
    stash_all.set_enabled(has_changed_files);

    let discard_all = add_menu_action(&actions, "discard-all", {
        let ctx = ctx.clone();
        move |_, _| discard_all_changes(&ctx)
    });
    discard_all.set_enabled(has_changed_files);

    menu.popup(parent, x, y, &actions, active_context_menu);
}

fn changed_file_selector_menu() -> context_menu::ContextMenuBuilder {
    context_menu::builder("changed_files")
        .item("Select All", "select-all")
        .item("Deselect All", "deselect-all")
        .separator()
        .item("Stash All Changes", "stash-all")
        .item("Discard All Changes...", "discard-all")
}

fn show_changed_file_context_menu(
    ctx: &PageContext,
    parent: &gtk::ListBox,
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
    file_path: &str,
    x: f64,
    y: f64,
) {
    let local_workspace = ctx.system_ref().provider_kind == ProviderKind::Local;
    let menu = changed_file_menu(file_path, ctx.files().is_some());
    let actions = changed_file_action_group(ctx, local_workspace);
    menu.popup(parent, x, y, &actions, active_context_menu);
}

fn changed_file_menu(
    file_path: &str,
    include_ignore_actions: bool,
) -> context_menu::ContextMenuBuilder {
    let mut menu = context_menu::builder("changed_file")
        .target_item("Open With Default Program", "open-default", file_path)
        .target_item("Open in Visual Studio Code", "open-code", file_path)
        .target_item("Show in File Manager", "show-folder", file_path)
        .target_item("Show in File View", "show-file-view", file_path);

    if include_ignore_actions {
        menu = append_changed_file_ignore_section(menu, file_path);
    }

    menu.separator()
        .target_item("Copy Path", "copy-path", file_path)
        .target_item("Copy Relative Path", "copy-relative-path", file_path)
        .separator()
        .target_item("Discard Changes...", "discard", file_path)
}

fn append_changed_file_ignore_section(
    mut menu: context_menu::ContextMenuBuilder,
    file_path: &str,
) -> context_menu::ContextMenuBuilder {
    let options = gitignore::options_for_path(file_path, IgnoreTargetKind::File);
    if options.direct.is_none() && options.folders.is_empty() && options.extension.is_none() {
        return menu;
    }

    menu = menu.separator();
    if let Some(option) = options.direct {
        menu = menu.target_item(&option.label, "ignore-pattern", &option.pattern);
    }
    if !options.folders.is_empty() {
        let mut folders = context_menu::builder("changed_file");
        for option in options.folders {
            folders = folders.target_item(&option.label, "ignore-pattern", &option.pattern);
        }
        menu = menu.submenu("Ignore Folder (Add to .gitignore)", &folders.build());
    }
    if let Some(option) = options.extension {
        menu = menu.target_item(&option.label, "ignore-pattern", &option.pattern);
    }
    menu
}

fn changed_file_action_group(ctx: &PageContext, local_workspace: bool) -> gio::SimpleActionGroup {
    let actions = gio::SimpleActionGroup::new();
    let desktop_open_available = local_workspace && ctx.desktop_opener().is_some();
    let parent_window = ctx.window().map(|window| window.upcast::<gtk::Window>());

    let open_default = context_menu::add_string_menu_action(&actions, "open-default", {
        let ctx = ctx.clone();
        let parent_window = parent_window.clone();
        move |file_path| {
            let Some(desktop_opener) = ctx.desktop_opener() else {
                ctx.show_error("Open Failed", &ctx.desktop_opener_unavailable_message());
                return;
            };
            let path = ctx.workspace_node_path(file_path);
            match desktop_opener.open_path(
                &path,
                DesktopOpenTargetKind::File,
                DesktopOpenActivation::from_parent(parent_window.as_ref()),
            ) {
                Ok(_) => ctx.refresh(Some("Opened file.".to_string())),
                Err(err) => ctx.show_error("Open Failed", &err),
            }
        }
    });
    open_default.set_enabled(desktop_open_available);
    let open_code = context_menu::add_string_menu_action(&actions, "open-code", {
        let ctx = ctx.clone();
        move |file_path| {
            let Some(repo_path) = ctx.local_workspace_path() else {
                ctx.show_error(
                    "Open Failed",
                    "Opening Visual Studio Code is unavailable for this workspace.",
                );
                return;
            };
            let target = repo_path.join(file_path);
            launch_path(&ctx, "code", &[target], "Opened in Visual Studio Code.");
        }
    });
    open_code.set_enabled(local_workspace);
    let show_folder = context_menu::add_string_menu_action(&actions, "show-folder", {
        let ctx = ctx.clone();
        let parent_window = parent_window.clone();
        move |file_path| {
            let Some(desktop_opener) = ctx.desktop_opener() else {
                ctx.show_error("Open Failed", &ctx.desktop_opener_unavailable_message());
                return;
            };
            let path = ctx.workspace_node_path(file_path);
            match desktop_opener.reveal_path(
                &path,
                DesktopOpenActivation::from_parent(parent_window.as_ref()),
            ) {
                Ok(_) => ctx.refresh(Some("Opened file manager.".to_string())),
                Err(err) => ctx.show_error("Open Failed", &err),
            }
        }
    });
    show_folder.set_enabled(desktop_open_available);
    context_menu::add_string_menu_action(&actions, "show-file-view", {
        let ctx = ctx.clone();
        move |file_path| {
            ctx.dispatch_command(PageCommand::OpenFileLocation {
                path: file_path.to_string(),
                line: None,
                column: None,
            });
        }
    });
    context_menu::add_string_menu_action(&actions, "copy-path", {
        let ctx = ctx.clone();
        move |file_path| {
            let workspace = ctx.workspace_ref();
            let path = ctx.workspace_node_path(file_path);
            let text = ctx
                .files()
                .map(|files| files.copy_path(&path))
                .unwrap_or_else(|| workspace.path(file_path).absolute);
            copy_to_clipboard(&ctx, &text);
        }
    });
    context_menu::add_string_menu_action(&actions, "copy-relative-path", {
        let ctx = ctx.clone();
        move |file_path| copy_to_clipboard(&ctx, file_path)
    });
    let ignore_pattern = context_menu::add_string_menu_action(&actions, "ignore-pattern", {
        let ctx = ctx.clone();
        move |pattern| ignore_pattern(&ctx, pattern)
    });
    ignore_pattern.set_enabled(ctx.files().is_some());
    context_menu::add_string_menu_action(&actions, "discard", {
        let ctx = ctx.clone();
        move |file_path| confirm_discard_changes(&ctx, vec![file_path.to_string()])
    });

    actions
}

fn discard_all_changes(ctx: &PageContext) {
    let ctx = ctx.clone();
    let workspace_key = ctx.workspace_key();
    let Some(git_handle) = ctx.git() else {
        ctx.show_error("Discard Failed", &ctx.git_unavailable_message());
        return;
    };
    ctx.clone().request_git_snapshot(
        workspace_key.clone(),
        git_handle,
        move |response_key, result| {
            if response_key != workspace_key || !ctx.workspace_is_current(&workspace_key) {
                return;
            }

            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    ctx.show_error("Discard Failed", &err);
                    return;
                }
            };

            if snapshot.changed_files.is_empty() {
                return;
            }

            confirm_discard_changes(
                &ctx,
                snapshot
                    .changed_files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect(),
            );
        },
    );
}

fn confirm_discard_changes(ctx: &PageContext, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let Some(window) = ctx.window() else {
        return;
    };

    let body = if paths.len() == 1 {
        format!(
            "Are you sure you want to discard all changes to:\n\n{}",
            paths[0]
        )
    } else {
        format!(
            "Are you sure you want to discard all changes to {} files?",
            paths.len()
        )
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Confirm Discard Changes")
        .body(&body)
        .build();
    dialog.add_response("discard", "Discard Changes");
    dialog.add_response("cancel", "Cancel");
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        move |response| {
            if response.as_str() != "discard" {
                return;
            }

            let Some(git_handle) = ctx.git() else {
                ctx.show_error("Discard Failed", &ctx.git_unavailable_message());
                return;
            };

            let message = if paths.len() == 1 {
                format!("Discarded {}.", paths[0])
            } else {
                "Discarded all changes.".to_string()
            };
            let completion = changes_git_unit_command(&ctx, "Discard Failed", message);
            discard_paths(
                git_handle,
                paths.clone(),
                Box::new(move |result| {
                    completion.send(result);
                }),
            );
        }
    });
}

fn discard_paths(
    git_handle: Arc<GitRepoHandle>,
    paths: Vec<String>,
    callback: OperationCallback<()>,
) {
    discard_path_at(git_handle, Arc::new(paths), 0, callback);
}

fn discard_path_at(
    git_handle: Arc<GitRepoHandle>,
    paths: Arc<Vec<String>>,
    index: usize,
    callback: OperationCallback<()>,
) {
    let Some(path) = paths.get(index).cloned() else {
        callback(Ok(()));
        return;
    };
    let next_handle = git_handle.clone();
    git_handle.discard_path(
        &path,
        Box::new(move |result| match result {
            Ok(_) => discard_path_at(next_handle, paths, index + 1, callback),
            Err(err) => callback(Err(err)),
        }),
    );
}

fn changes_git_string_command(
    ctx: &PageContext,
    error_heading: &'static str,
    empty_success_message: Option<String>,
) -> command_mailbox::UiCommandSender<Result<String, String>> {
    let ctx = ctx.clone();
    command_mailbox::once(move |result: Result<String, String>| match result {
        Ok(output) => {
            let message = if output.is_empty() {
                empty_success_message.clone().unwrap_or_default()
            } else {
                output
            };
            ctx.refresh((!message.is_empty()).then_some(message));
        }
        Err(err) => ctx.show_error(error_heading, &err),
    })
}

fn changes_git_unit_command(
    ctx: &PageContext,
    error_heading: &'static str,
    success_message: String,
) -> command_mailbox::UiCommandSender<Result<(), String>> {
    let ctx = ctx.clone();
    command_mailbox::once(move |result: Result<(), String>| match result {
        Ok(()) => {
            ctx.refresh(Some(success_message.clone()));
        }
        Err(err) => ctx.show_error(error_heading, &err),
    })
}

fn ignore_pattern(ctx: &PageContext, pattern: &str) {
    let Some(files) = ctx.files() else {
        ctx.show_error(
            "Ignore Failed",
            "File access is unavailable for this workspace.",
        );
        return;
    };
    let ctx = ctx.clone();
    let completion = command_mailbox::once(move |result: Result<String, String>| match result {
        Ok(message) => ctx.refresh(Some(message)),
        Err(err) => ctx.show_error("Ignore Failed", &err),
    });
    gitignore::add_pattern_to_workspace(
        files,
        pattern.to_string(),
        Box::new(move |result| {
            completion.send(result);
        }),
    );
}

fn open_repository_in_files(ctx: &PageContext) {
    let Some(desktop_opener) = ctx.desktop_opener() else {
        ctx.show_error("Open Failed", &ctx.desktop_opener_unavailable_message());
        return;
    };
    let path = ctx.workspace_root_node_path();
    let parent_window = ctx.window().map(|window| window.upcast::<gtk::Window>());

    match desktop_opener.open_path(
        &path,
        DesktopOpenTargetKind::Folder,
        DesktopOpenActivation::from_parent(parent_window.as_ref()),
    ) {
        Ok(_) => ctx.refresh(Some("Opened in Files.".to_string())),
        Err(err) => ctx.show_error("Open Failed", &err),
    }
}

fn open_repository_in_ghostty(ctx: &PageContext, event_time: u32) {
    let Some(repo_path) = ctx.local_workspace_path() else {
        ctx.show_error(
            "Open Failed",
            "Opening Ghostty is unavailable for this workspace.",
        );
        return;
    };
    let Some(window) = ctx.window() else {
        ctx.show_error("Open Failed", "Application window is not available.");
        return;
    };

    match launch_ghostty(&window, &repo_path, event_time) {
        Ok(()) => {
            log::info!(
                "opened repository in external Ghostty path={}",
                repo_path.display()
            );
            ctx.refresh(Some("Opened in Ghostty.".to_string()));
        }
        Err(err) => {
            log::warn!(
                "failed to open repository in external Ghostty path={} error={err}",
                repo_path.display()
            );
            ctx.show_error("Open Failed", &err);
        }
    }
}

fn launch_ghostty(
    window: &adw::ApplicationWindow,
    repo_path: &Path,
    event_time: u32,
) -> Result<(), String> {
    let context = file_manager::app_launch_context(window, event_time);
    let repo_path = repo_path.to_string_lossy();
    let commandline = format!(
        "ghostty --working-directory={}",
        shell_quote(repo_path.as_ref())
    );
    let app = gio::AppInfo::create_from_commandline(
        commandline,
        Some("Ghostty"),
        gio::AppInfoCreateFlags::SUPPORTS_STARTUP_NOTIFICATION,
    )
    .map_err(|err| format!("Failed to prepare Ghostty launch: {err}"))?;

    app.launch(&[] as &[gio::File], Some(&context))
        .map_err(|err| format!("Failed to run ghostty: {err}"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn launch_repo_command(ctx: &PageContext, program: &str, args: &[&str], success_message: &str) {
    let Some(repo_path) = ctx.local_workspace_path() else {
        ctx.show_error(
            "Open Failed",
            &format!("Running {program} is unavailable for this workspace."),
        );
        return;
    };
    let mut cmd = Command::new(program);

    if let Some(last_arg) = args.last() {
        if last_arg.ends_with('=') {
            if args.len() > 1 {
                cmd.args(&args[0..args.len() - 1]);
            }
            cmd.arg(format!("{}{}", last_arg, repo_path.display()));
        } else {
            cmd.args(args);
            cmd.arg(&repo_path);
        }
    } else {
        cmd.arg(&repo_path);
    }

    match cmd.spawn() {
        Ok(_) => ctx.refresh(Some(success_message.to_string())),
        Err(err) => ctx.show_error("Open Failed", &format!("Failed to run {program}: {err}")),
    }
}

fn launch_path(ctx: &PageContext, program: &str, paths: &[PathBuf], success_message: &str) {
    match Command::new(program).args(paths).spawn() {
        Ok(_) => ctx.refresh(Some(success_message.to_string())),
        Err(err) => ctx.show_error("Open Failed", &format!("Failed to run {program}: {err}")),
    }
}

fn open_remote_repository(ctx: &PageContext) {
    let ctx = ctx.clone();
    let workspace_key = ctx.workspace_key();
    let Some(git_handle) = ctx.git() else {
        ctx.show_error("Open Remote Failed", &ctx.git_unavailable_message());
        return;
    };
    ctx.clone().request_git_snapshot(
        workspace_key.clone(),
        git_handle,
        move |response_key, result| {
            if response_key != workspace_key || !ctx.workspace_is_current(&workspace_key) {
                return;
            }

            let remote_url = match result {
                Ok(snapshot) => snapshot.remote_url,
                Err(err) => {
                    ctx.show_error("Open Remote Failed", &err);
                    return;
                }
            };

            let Some(remote_url) = remote_url else {
                ctx.show_error("No Remote", "No remote URL configured.");
                return;
            };

            let url = git::remote_web_url(&remote_url);
            let Some(url_opener) = ctx.url_opener() else {
                ctx.show_error("Open Failed", &ctx.url_opener_unavailable_message());
                return;
            };
            match url_opener.open_url(&url, UrlOpenActivation::default()) {
                Ok(_) => ctx.refresh(Some(format!("Opened {url}."))),
                Err(err) => ctx.show_error("Open Failed", &err),
            }
        },
    );
}

fn copy_to_clipboard(ctx: &PageContext, text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    } else {
        ctx.show_error("Copy Failed", "No display clipboard is available.");
    }
}

fn add_menu_action<F>(group: &gio::SimpleActionGroup, name: &str, activate: F) -> gio::SimpleAction
where
    F: Fn(&gio::SimpleAction, Option<&gtk::glib::Variant>) + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |action, parameter| activate(action, parameter));
    group.add_action(&action);
    action
}

fn track_button_event_time(button: &gtk::Button) -> Rc<Cell<u32>> {
    let event_time = Rc::new(Cell::new(0));
    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let event_time = event_time.clone();
        move |gesture, _, _, _| event_time.set(gesture.current_event_time())
    });
    button.add_controller(click);
    event_time
}
