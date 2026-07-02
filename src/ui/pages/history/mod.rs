use super::{Page, PageCommand, PageCommandResult, PageContext};
use crate::git::{self, BytesComparison, FileComparison, RepositorySnapshot};
use crate::system::capabilities::git::GitAccess;
use crate::system::capabilities::url::UrlOpenActivation;
use crate::ui::components::context_menu;
use crate::ui::file_type::PreviewKind;
use crate::ui::request_provider_git_snapshot;
use crate::ui::sidebar::history::HistoryList;
use adw::prelude::*;
use gtk::gio;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

mod right;

const HISTORY_PREVIEW_CACHE_LIMIT: usize = 48;

#[derive(Clone, Debug)]
enum HistoryFilePreview {
    Diff(FileComparison),
    Bytes(BytesComparison),
    PreviewLimit(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryPreviewKey {
    hash: String,
    path: String,
    kind: PreviewKind,
}

struct HistoryPreviewWorkerResult {
    key: HistoryPreviewKey,
    result: Result<HistoryFilePreview, String>,
    duration: Duration,
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

    fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }
}

pub(super) struct HistoryPage {
    ctx: PageContext,
    left: HistoryList,
    right: Rc<right::HistoryRight>,
    selected_commit: Rc<RefCell<Option<String>>>,
    preview_cache: Rc<RefCell<BoundedPreviewCache<HistoryPreviewKey, HistoryFilePreview>>>,
    preview_workspace_key: Rc<RefCell<Option<String>>>,
    active_context_menu: Rc<RefCell<Option<gtk::Popover>>>,
}

impl HistoryPage {
    pub(super) fn new(ctx: PageContext) -> Self {
        let page = Self {
            ctx,
            left: HistoryList::new(),
            right: Rc::new(right::HistoryRight::new()),
            selected_commit: Rc::new(RefCell::new(None)),
            preview_cache: Rc::new(RefCell::new(BoundedPreviewCache::new(
                HISTORY_PREVIEW_CACHE_LIMIT,
            ))),
            preview_workspace_key: Rc::new(RefCell::new(None)),
            active_context_menu: Rc::new(RefCell::new(None)),
        };
        page.connect_commit_selection();
        page.connect_file_selection();
        page.connect_context_menu();
        page
    }

    fn connect_commit_selection(&self) {
        self.left.connect_selected({
            let ctx = self.ctx.clone();
            let left = self.left.clone();
            let right = self.right.clone();
            let selected_commit = self.selected_commit.clone();

            move || {
                let Some(hash) = left.selected_commit_hash() else {
                    selected_commit.borrow_mut().take();
                    right.show_empty();
                    return;
                };

                ctx.dispatch_command(PageCommand::OpenCommit(hash));
            }
        });
    }

    fn connect_file_selection(&self) {
        self.right.connect_file_selected({
            let ctx = self.ctx.clone();
            let right = self.right.clone();
            let selected_commit = self.selected_commit.clone();
            let preview_cache = self.preview_cache.clone();

            move |file_path| {
                let Some(file_path) = file_path else {
                    return;
                };
                let Some(hash) = selected_commit.borrow().clone() else {
                    return;
                };

                show_history_file_preview(
                    &ctx,
                    &right,
                    &selected_commit,
                    &preview_cache,
                    hash,
                    file_path,
                );
            }
        });
    }

    fn connect_context_menu(&self) {
        let ctx = self.ctx.clone();
        let active_context_menu = self.active_context_menu.clone();
        self.left
            .connect_context_requested(move |parent, hash, x, y, event_time| {
                show_history_commit_context_menu(
                    &ctx,
                    parent,
                    &active_context_menu,
                    hash,
                    x,
                    y,
                    event_time,
                );
            });
    }
}

impl Page for HistoryPage {
    fn label(&self) -> &'static str {
        "History"
    }

    fn icon_name(&self) -> &'static str {
        "document-open-recent-symbolic"
    }

    fn left(&self) -> gtk::Widget {
        self.left.root.clone().upcast()
    }

    fn right(&self) -> gtk::Widget {
        self.right.root()
    }

    fn activate(&self) {
        if self.selected_commit.borrow().is_none() {
            self.left.ensure_loaded();
        }
    }

    fn refresh(&self, snapshot: &RepositorySnapshot) {
        sync_history_preview_workspace(
            &self.preview_workspace_key,
            &self.preview_cache,
            &self.ctx.workspace_key(),
        );
        self.left
            .update(snapshot, self.ctx.workspace_key(), self.ctx.git());
        if snapshot.history_head.is_none() {
            self.selected_commit.borrow_mut().take();
            clear_history_preview_cache(
                &self.preview_cache,
                "no-history",
                &self.ctx.workspace_key(),
            );
            self.right.show_empty();
        }
    }

    fn set_error(&self, message: &str) {
        self.selected_commit.borrow_mut().take();
        clear_history_preview_cache(
            &self.preview_cache,
            "repository-error",
            &self.ctx.workspace_key(),
        );
        self.left.clear();
        self.right.show_error(message);
    }

    fn toggle_left_search(&self) -> bool {
        self.left.toggle_search();
        true
    }

    fn toggle_right_search(&self) -> bool {
        self.right.toggle_search()
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        match command {
            PageCommand::OpenCommit(hash) => {
                show_history_commit(&self.ctx, &self.right, &self.selected_commit, hash.clone());
                PageCommandResult::HandledAndActivate
            }
            PageCommand::ClearSelection => {
                self.selected_commit.borrow_mut().take();
                self.right.show_empty();
                PageCommandResult::Handled
            }
            PageCommand::OpenSearchMatch { .. }
            | PageCommand::OpenFileLocation { .. }
            | PageCommand::AddFileToAgent(_)
            | PageCommand::OpenAgentSession(_) => PageCommandResult::Ignored,
        }
    }
}

fn show_history_commit(
    ctx: &PageContext,
    right: &Rc<right::HistoryRight>,
    selected_commit: &Rc<RefCell<Option<String>>>,
    hash: String,
) {
    let Some(git_access) = ctx.git() else {
        ctx.show_error("Commit Failed", &ctx.git_unavailable_message());
        return;
    };
    let (sender, receiver) = mpsc::channel();

    *selected_commit.borrow_mut() = Some(hash.clone());

    thread::spawn({
        let hash = hash.clone();

        move || {
            let result = git_access.commit_details(&hash).and_then(|commit| {
                git_access
                    .commit_changed_files(&hash)
                    .map(|files| (commit, files))
            });
            let _ = sender.send(result);
        }
    });

    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let ctx = ctx.clone();
        let right = right.clone();
        let selected_commit = selected_commit.clone();

        move || match receiver.try_recv() {
            Ok(Ok((commit, files))) => {
                if selected_commit.borrow().as_deref() == Some(hash.as_str()) {
                    right.show_commit(&commit, &files);
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                if selected_commit.borrow().as_deref() == Some(hash.as_str()) {
                    selected_commit.borrow_mut().take();
                    right.show_empty();
                    ctx.show_error("Commit Failed", &err);
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if selected_commit.borrow().as_deref() == Some(hash.as_str()) {
                    selected_commit.borrow_mut().take();
                    right.show_empty();
                    ctx.show_error("Commit Failed", "Commit loading did not return a result.");
                }
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn show_history_file_preview(
    ctx: &PageContext,
    right: &Rc<right::HistoryRight>,
    selected_commit: &Rc<RefCell<Option<String>>>,
    preview_cache: &Rc<RefCell<BoundedPreviewCache<HistoryPreviewKey, HistoryFilePreview>>>,
    hash: String,
    file_path: String,
) {
    let kind = crate::ui::file_type::preview_kind_for_path(&file_path, false);
    let key = HistoryPreviewKey {
        hash,
        path: file_path,
        kind,
    };

    if let Some(preview) = preview_cache.borrow_mut().get(&key) {
        log::info!(
            "history preview cache hit workspace={} hash={} path={} kind={:?} {}",
            ctx.workspace_key(),
            key.hash.as_str(),
            key.path,
            key.kind,
            history_preview_summary(&preview)
        );
        show_history_preview_outcome(right, &key.path, &preview);
        return;
    }

    log::info!(
        "history preview cache miss workspace={} hash={} path={} kind={:?}",
        ctx.workspace_key(),
        key.hash.as_str(),
        key.path,
        key.kind
    );

    let Some(git_access) = ctx.git() else {
        ctx.show_error("Diff Failed", &ctx.git_unavailable_message());
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let workspace_key = ctx.workspace_key();

    thread::spawn({
        let key = key.clone();

        move || {
            let start = Instant::now();
            let result = commit_file_preview(git_access.as_ref(), &key);
            let _ = sender.send(HistoryPreviewWorkerResult {
                key,
                result,
                duration: start.elapsed(),
            });
        }
    });

    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let ctx = ctx.clone();
        let right = right.clone();
        let selected_commit = selected_commit.clone();
        let preview_cache = preview_cache.clone();

        move || match receiver.try_recv() {
            Ok(result) => {
                if !ctx.workspace_is_current(&workspace_key)
                    || !is_current_history_file_selection(
                        &right,
                        &selected_commit,
                        &result.key.hash,
                        &result.key.path,
                    )
                {
                    log::info!(
                        "history preview stale result dropped workspace={} hash={} path={} kind={:?} duration_ms={}",
                        workspace_key,
                        result.key.hash.as_str(),
                        result.key.path,
                        result.key.kind,
                        result.duration.as_millis()
                    );
                    return gtk::glib::ControlFlow::Break;
                }

                match result.result {
                    Ok(preview) => {
                        log::info!(
                            "history preview loaded workspace={} hash={} path={} kind={:?} duration_ms={} {}",
                            workspace_key,
                            result.key.hash.as_str(),
                            result.key.path,
                            result.key.kind,
                            result.duration.as_millis(),
                            history_preview_summary(&preview)
                        );
                        let evicted = preview_cache
                            .borrow_mut()
                            .insert(result.key.clone(), preview.clone());
                        if evicted > 0 {
                            log::info!(
                                "history preview cache invalidation workspace={} reason=evict count={}",
                                workspace_key,
                                evicted
                            );
                        }
                        show_history_preview_outcome(&right, &result.key.path, &preview);
                    }
                    Err(err) => {
                        log::warn!(
                            "history preview load failed workspace={} hash={} path={} kind={:?} duration_ms={} err={}",
                            workspace_key,
                            result.key.hash.as_str(),
                            result.key.path,
                            result.key.kind,
                            result.duration.as_millis(),
                            err
                        );
                        ctx.show_error("Diff Failed", &err);
                    }
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if ctx.workspace_is_current(&workspace_key)
                    && is_current_history_file_selection(
                        &right,
                        &selected_commit,
                        &key.hash,
                        &key.path,
                    )
                {
                    ctx.show_error("Diff Failed", "Diff loading did not return a result.");
                }
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn commit_file_preview(
    git_access: &dyn GitAccess,
    key: &HistoryPreviewKey,
) -> Result<HistoryFilePreview, String> {
    let result = match key.kind {
        PreviewKind::Image
        | PreviewKind::Audio
        | PreviewKind::Video
        | PreviewKind::Font
        | PreviewKind::Pdf => git_access
            .commit_bytes_comparison(&key.hash, &key.path)
            .map(HistoryFilePreview::Bytes),
        _ => git_access
            .commit_comparison(&key.hash, &key.path)
            .map(HistoryFilePreview::Diff),
    };

    match result {
        Ok(preview) => Ok(preview),
        Err(err) if is_preview_limit_message(&err) => Ok(HistoryFilePreview::PreviewLimit(err)),
        Err(err) => Err(err),
    }
}

fn show_history_preview_outcome(
    right: &right::HistoryRight,
    file_path: &str,
    preview: &HistoryFilePreview,
) {
    match preview {
        HistoryFilePreview::Diff(comparison) => right.show_comparison(file_path, comparison),
        HistoryFilePreview::Bytes(comparison) => {
            right.show_binary_comparison(file_path, comparison)
        }
        HistoryFilePreview::PreviewLimit(message) => {
            right.show_preview_unavailable(file_path, message);
        }
    }
}

fn history_preview_summary(preview: &HistoryFilePreview) -> String {
    match preview {
        HistoryFilePreview::Diff(comparison) => format!("rows={}", comparison.rows.len()),
        HistoryFilePreview::Bytes(comparison) => format!(
            "before_bytes={} after_bytes={}",
            comparison.before.as_ref().map(Vec::len).unwrap_or(0),
            comparison.after.as_ref().map(Vec::len).unwrap_or(0)
        ),
        HistoryFilePreview::PreviewLimit(_) => "preview_limit=true".to_string(),
    }
}

fn sync_history_preview_workspace(
    preview_workspace_key: &Rc<RefCell<Option<String>>>,
    preview_cache: &Rc<RefCell<BoundedPreviewCache<HistoryPreviewKey, HistoryFilePreview>>>,
    workspace_key: &str,
) {
    if preview_workspace_key.borrow().as_deref() == Some(workspace_key) {
        return;
    }

    let previous = preview_workspace_key.replace(Some(workspace_key.to_string()));
    let invalidated = preview_cache.borrow_mut().clear();
    log::info!(
        "history preview cache invalidation workspace={} previous_workspace={:?} reason=workspace-change count={}",
        workspace_key,
        previous,
        invalidated
    );
}

fn clear_history_preview_cache(
    preview_cache: &Rc<RefCell<BoundedPreviewCache<HistoryPreviewKey, HistoryFilePreview>>>,
    reason: &str,
    workspace_key: &str,
) {
    let invalidated = preview_cache.borrow_mut().clear();
    if invalidated > 0 {
        log::info!(
            "history preview cache invalidation workspace={} reason={} count={}",
            workspace_key,
            reason,
            invalidated
        );
    }
}

fn is_preview_limit_message(message: &str) -> bool {
    message.contains("too large to preview") || message.contains("cannot be previewed as text")
}

fn is_current_history_file_selection(
    right: &right::HistoryRight,
    selected_commit: &Rc<RefCell<Option<String>>>,
    hash: &str,
    file_path: &str,
) -> bool {
    selected_commit.borrow().as_deref() == Some(hash)
        && right.selected_file_path().as_deref() == Some(file_path)
}

fn show_history_commit_context_menu(
    ctx: &PageContext,
    parent: &gtk::Box,
    active_context_menu: &Rc<RefCell<Option<gtk::Popover>>>,
    hash: String,
    x: f64,
    y: f64,
    _event_time: u32,
) {
    let Some(git_access) = ctx.git() else {
        ctx.show_error("History Action Failed", &ctx.git_unavailable_message());
        return;
    };
    let workspace_key = ctx.workspace_key();
    let ctx = ctx.clone();
    let parent = parent.clone();
    let active_context_menu = active_context_menu.clone();
    let (sender, receiver) = mpsc::channel();

    thread::spawn({
        let git_access = git_access.clone();
        let hash = hash.clone();
        move || {
            let parent_hash = git_access.commit_parent_hash(&hash).ok().flatten();
            let snapshot = git_access.snapshot();
            let _ = sender.send((parent_hash, snapshot));
        }
    });

    gtk::glib::timeout_add_local(Duration::from_millis(75), move || {
        match receiver.try_recv() {
            Ok((parent_hash, result)) => {
                if !ctx.workspace_is_current(&workspace_key) {
                    return gtk::glib::ControlFlow::Break;
                }
                let has_remote = result
                    .ok()
                    .and_then(|snapshot| snapshot.remote_url)
                    .is_some();
                let actions = history_commit_action_group(
                    &ctx,
                    hash.clone(),
                    parent_hash.clone(),
                    has_remote,
                );
                history_commit_menu().popup(&parent, x, y, &actions, &active_context_menu);
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if ctx.workspace_is_current(&workspace_key) {
                    ctx.show_error(
                        "History Action Failed",
                        "Commit action loading did not return a result.",
                    );
                }
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn history_commit_action_group(
    ctx: &PageContext,
    hash: String,
    parent_hash: Option<String>,
    has_remote: bool,
) -> gio::SimpleActionGroup {
    let actions = gio::SimpleActionGroup::new();

    add_menu_action(&actions, "checkout", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| {
            let hash = hash.clone();
            run_history_git_action(&ctx, "Checkout Failed", "Checked out commit.", move |git| {
                git.checkout_commit(&hash)
            });
        }
    });

    let checkout_parent = add_menu_action(&actions, "checkout-parent", {
        let ctx = ctx.clone();
        let parent_hash = parent_hash.clone();
        move |_, _| match parent_hash.clone() {
            Some(parent_hash) => {
                run_history_git_action(
                    &ctx,
                    "Checkout Failed",
                    "Checked out parent commit.",
                    move |git| git.checkout_commit(&parent_hash),
                );
            }
            None => ctx.show_error("Checkout Failed", "This commit has no parent."),
        }
    });
    checkout_parent.set_enabled(parent_hash.is_some());

    add_menu_action(&actions, "branch", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| prompt_create_branch_at_commit(&ctx, hash.clone())
    });

    add_menu_action(&actions, "tag", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| prompt_create_tag_at_commit(&ctx, hash.clone())
    });

    add_menu_action(&actions, "cherry-pick", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| {
            let hash = hash.clone();
            run_history_git_action(
                &ctx,
                "Cherry-Pick Failed",
                "Cherry-picked commit.",
                move |git| git.cherry_pick_commit(&hash),
            );
        }
    });

    add_menu_action(&actions, "revert", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| confirm_revert_commit(&ctx, hash.clone())
    });

    add_menu_action(&actions, "amend-head", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| prompt_amend_head_from_commit(&ctx, hash.clone())
    });

    add_menu_action(&actions, "reset-mixed", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| confirm_reset_to_commit(&ctx, hash.clone(), git::ResetMode::Mixed)
    });

    add_menu_action(&actions, "reset-hard", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| confirm_reset_to_commit(&ctx, hash.clone(), git::ResetMode::Hard)
    });

    let open_remote = add_menu_action(&actions, "open-remote", {
        let ctx = ctx.clone();
        let hash = hash.clone();
        move |_, _| open_remote_commit(&ctx, &hash)
    });
    open_remote.set_enabled(has_remote);

    actions
}

fn history_commit_menu() -> context_menu::ContextMenuBuilder {
    context_menu::builder("history_commit")
        .item("Checkout Commit", "checkout")
        .item("Checkout Parent", "checkout-parent")
        .separator()
        .item("New Branch Here...", "branch")
        .item("Create Tag...", "tag")
        .separator()
        .item("Cherry-Pick Commit", "cherry-pick")
        .item("Revert Commit...", "revert")
        .separator()
        .item("Amend HEAD With This Message...", "amend-head")
        .item("Reset Current Branch Here (--mixed)...", "reset-mixed")
        .item("Reset Current Branch Here (--hard)...", "reset-hard")
        .separator()
        .item("Open Commit on Remote", "open-remote")
}

fn prompt_create_branch_at_commit(ctx: &PageContext, hash: String) {
    let Some(window) = ctx.window() else {
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("New Branch")
        .body("Enter a branch name for this commit.")
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create Branch");
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let entry = entry.clone();
        move |response| {
            if response.as_str() != "create" {
                return;
            }

            let branch = entry.text().trim().to_string();
            if branch.is_empty() {
                ctx.show_error("Create Branch Failed", "Enter a branch name.");
                return;
            }

            let fallback = format!("Created and checked out {branch}.");
            let hash = hash.clone();
            run_history_git_action(&ctx, "Create Branch Failed", &fallback, move |git| {
                git.create_branch_at_commit(&branch, &hash)
            });
        }
    });
}

fn prompt_create_tag_at_commit(ctx: &PageContext, hash: String) {
    let Some(window) = ctx.window() else {
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Create Tag")
        .body("Enter a tag name for this commit.")
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("Tag name")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create Tag");
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let entry = entry.clone();
        move |response| {
            if response.as_str() != "create" {
                return;
            }

            let tag = entry.text().trim().to_string();
            if tag.is_empty() {
                ctx.show_error("Create Tag Failed", "Enter a tag name.");
                return;
            }

            let fallback = format!("Created tag {tag}.");
            let hash = hash.clone();
            run_history_git_action(&ctx, "Create Tag Failed", &fallback, move |git| {
                git.create_tag(&tag, &hash)
            });
        }
    });
}

fn confirm_revert_commit(ctx: &PageContext, hash: String) {
    let body = "This creates a new commit that reverses the selected commit.";
    confirm_history_action(
        ctx,
        "Revert Commit",
        body,
        "revert",
        "Revert Commit",
        None,
        {
            let ctx = ctx.clone();
            let hash = hash.clone();
            move || {
                let hash = hash.clone();
                run_history_git_action(&ctx, "Revert Failed", "Reverted commit.", move |git| {
                    git.revert_commit(&hash)
                });
            }
        },
    );
}

fn confirm_reset_to_commit(ctx: &PageContext, hash: String, mode: git::ResetMode) {
    let (heading, body, response_label, fallback) = match mode {
        git::ResetMode::Mixed => (
            "Reset Current Branch",
            "This moves the current branch to the selected commit and leaves file changes in the working tree.",
            "Reset --mixed",
            "Reset current branch.",
        ),
        git::ResetMode::Hard => (
            "Hard Reset Current Branch",
            "This moves the current branch to the selected commit and discards working tree changes.",
            "Reset --hard",
            "Hard reset current branch.",
        ),
    };

    confirm_history_action(
        ctx,
        heading,
        body,
        "reset",
        response_label,
        Some(adw::ResponseAppearance::Destructive),
        {
            let ctx = ctx.clone();
            let hash = hash.clone();
            move || {
                let hash = hash.clone();
                run_history_git_action(&ctx, "Reset Failed", fallback, move |git| {
                    git.reset_to_commit(&hash, mode)
                });
            }
        },
    );
}

fn prompt_amend_head_from_commit(ctx: &PageContext, hash: String) {
    let Some(window) = ctx.window() else {
        return;
    };
    let Some(git_access) = ctx.git() else {
        ctx.show_error("Load Commit Message Failed", &ctx.git_unavailable_message());
        return;
    };
    let message = match git_access.commit_message(&hash) {
        Ok(message) => message,
        Err(err) => {
            ctx.show_error("Load Commit Message Failed", &err);
            return;
        }
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Amend HEAD")
        .body(
            "Edit the message for HEAD. The selected commit message is used as the starting point.",
        )
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let summary_entry = gtk::Entry::builder()
        .text(&message.summary)
        .placeholder_text("Summary")
        .activates_default(true)
        .build();
    let description_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .vexpand(true)
        .build();
    description_view.buffer().set_text(&message.description);
    let description_scroller = gtk::ScrolledWindow::builder()
        .min_content_height(160)
        .child(&description_view)
        .build();
    content.append(&summary_entry);
    content.append(&description_scroller);
    dialog.set_extra_child(Some(&content));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("amend", "Amend HEAD");
    dialog.set_response_appearance("amend", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("amend"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let summary_entry = summary_entry.clone();
        let description_view = description_view.clone();
        move |response| {
            if response.as_str() != "amend" {
                return;
            }

            let summary = summary_entry.text().trim().to_string();
            let description = text_view_text(&description_view);
            run_history_git_action(&ctx, "Amend Failed", "Amended HEAD.", move |git| {
                git.amend_head(&summary, &description)
            });
        }
    });
}

fn open_remote_commit(ctx: &PageContext, hash: &str) {
    let ctx = ctx.clone();
    let hash = hash.to_string();
    let workspace_key = ctx.workspace_key();
    let Some(git_access) = ctx.git() else {
        ctx.show_error("Open Remote Commit Failed", &ctx.git_unavailable_message());
        return;
    };
    request_provider_git_snapshot(
        workspace_key.clone(),
        git_access,
        move |response_key, result| {
            if response_key != workspace_key || !ctx.workspace_is_current(&workspace_key) {
                return;
            }

            let remote_url = match result {
                Ok(snapshot) => snapshot.remote_url,
                Err(err) => {
                    ctx.show_error("Open Remote Commit Failed", &err);
                    return;
                }
            };

            let Some(remote_url) = remote_url else {
                ctx.show_error(
                    "Open Remote Commit Failed",
                    "No remote URL is configured for this repository.",
                );
                return;
            };

            let url = git::remote_commit_web_url(&remote_url, &hash);
            let Some(url_opener) = ctx.url_opener() else {
                ctx.show_error(
                    "Open Remote Commit Failed",
                    &ctx.url_opener_unavailable_message(),
                );
                return;
            };
            if let Err(err) = url_opener.open_url(&url, UrlOpenActivation::default()) {
                ctx.show_error("Open Remote Commit Failed", &err);
                return;
            }

            ctx.refresh_without_toast(None);
        },
    );
}

fn confirm_history_action<F>(
    ctx: &PageContext,
    heading: &str,
    body: &str,
    response_id: &str,
    response_label: &str,
    response_appearance: Option<adw::ResponseAppearance>,
    action: F,
) where
    F: Fn() + 'static,
{
    let Some(window) = ctx.window() else {
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response(response_id, response_label);
    if let Some(response_appearance) = response_appearance {
        dialog.set_response_appearance(response_id, response_appearance);
    }
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let response_id = response_id.to_string();
        move |response| {
            if response.as_str() == response_id {
                action();
            }
        }
    });
}

fn finish_history_git_action(
    ctx: &PageContext,
    error_heading: &str,
    result: Result<String, String>,
    fallback_message: &str,
) {
    match result {
        Ok(output) => {
            let message = if output.trim().is_empty() {
                fallback_message.to_string()
            } else {
                output
            };
            ctx.refresh(Some(message));
        }
        Err(err) => ctx.show_error(error_heading, &err),
    }
}

fn run_history_git_action<F>(
    ctx: &PageContext,
    error_heading: &str,
    fallback_message: &str,
    action: F,
) where
    F: FnOnce(Arc<dyn GitAccess>) -> Result<String, String> + Send + 'static,
{
    let Some(git_access) = ctx.git() else {
        ctx.show_error(error_heading, &ctx.git_unavailable_message());
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let error_heading = error_heading.to_string();
    let fallback_message = fallback_message.to_string();

    thread::spawn(move || {
        let result = action(git_access);
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let ctx = ctx.clone();
        move || match receiver.try_recv() {
            Ok(result) => {
                finish_history_git_action(&ctx, &error_heading, result, &fallback_message);
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                ctx.show_error(&error_heading, "Git action did not return a result.");
                gtk::glib::ControlFlow::Break
            }
        }
    });
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

fn text_view_text(text_view: &gtk::TextView) -> String {
    let buffer = text_view.buffer();
    let (start, end) = buffer.bounds();
    buffer.text(&start, &end, true).to_string()
}
