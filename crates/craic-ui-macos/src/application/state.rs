struct HistoryUi {
    sidebar_root: Retained<NSView>,
    search: Retained<NSSearchField>,
    table: Retained<NSTableView>,
    menu: Retained<NSMenu>,
    scroll: Retained<NSScrollView>,
    loading_spinner: Retained<NSProgressIndicator>,
    status: Retained<NSTextField>,
    content_root: Retained<NSView>,
    title: Retained<NSTextField>,
    avatar: Retained<NSImageView>,
    metadata: Retained<NSTextField>,
    added: Retained<NSTextField>,
    deleted: Retained<NSTextField>,
    comment: Retained<NSTextField>,
    copy_hash: Retained<NSButton>,
    open_remote: Retained<NSButton>,
    files_table: Retained<NSTableView>,
    files_scroll: Retained<NSScrollView>,
    file_count: Retained<NSTextField>,
    diff: Retained<DiffMetalView>,
    binary_preview: Retained<NSView>,
    binary_font_registrations: RefCell<Vec<NativeFontRegistration>>,
    preview_cache: RefCell<VecDeque<CachedHistoryPreview>>,
    empty: Retained<NSTextField>,
    commits: RefCell<Vec<Commit>>,
    files: RefCell<Vec<ChangedFile>>,
    query: RefCell<String>,
    cursor: RefCell<Option<String>>,
    selected_hash: RefCell<Option<String>>,
    selected_commit: RefCell<Option<Commit>>,
    selected_parent_hash: RefCell<Option<String>>,
    parent_loaded: Cell<bool>,
    pending_checkout_parent: Cell<bool>,
    pending_amend: Cell<bool>,
    detail_loading: Cell<bool>,
    selected_file: RefCell<Option<String>>,
    loaded_diff_path: RefCell<Option<String>>,
    loaded_binary_path: RefCell<Option<String>>,
    avatar_source: RefCell<Option<String>>,
    has_more: Cell<bool>,
    loading: Cell<bool>,
    pending_search: Cell<bool>,
    generation: Cell<u64>,
    detail_request_id: Cell<u64>,
    comparison_request_id: Cell<u64>,
    action_in_progress: Cell<bool>,
}
struct FilesTableViewIvars {
    delegate: RefCell<Weak<AppDelegate>>,
}

define_class!(
    // SAFETY: The Files source-list table and its delegate are owned by the AppKit main thread.
    #[unsafe(super = NSTableView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = FilesTableViewIvars]
    struct FilesTableView;

    unsafe impl NSObjectProtocol for FilesTableView {}

    impl FilesTableView {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let modifiers = event.modifierFlags();
            let unmodified = event
                .charactersIgnoringModifiers()
                .map(|text| text.to_string())
                .unwrap_or_default();
            if modifiers.contains(NSEventModifierFlags::Command)
                && !modifiers
                    .intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
            {
                let Some(delegate) = self.ivars().delegate.borrow().load() else {
                    return;
                };
                match unmodified.as_str() {
                    "c" => delegate.store_workspace_file_clipboard(false),
                    "x" => delegate.store_workspace_file_clipboard(true),
                    "v" => delegate.paste_workspace_file_from_clipboard(),
                    _ => unsafe {
                        let _: () = msg_send![super(self), keyDown: event];
                    },
                }
                return;
            }
            if !modifiers.intersects(
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option,
            ) && let Some(delegate) = self.ivars().delegate.borrow().load()
            {
                match event.keyCode() {
                    36 | 76 => {
                        delegate.open_selected_workspace_entry();
                        return;
                    }
                    51 | 117 => {
                        delegate.confirm_delete_workspace_file();
                        return;
                    }
                    _ => {}
                }
            }
            unsafe {
                let _: () = msg_send![super(self), keyDown: event];
            }
        }
    }
);

impl FilesTableView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FilesTableViewIvars {
            delegate: RefCell::new(Weak::default()),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn attach_delegate(&self, delegate: &AppDelegate) {
        self.ivars().delegate.replace(Weak::new(delegate));
    }
}

struct ContainersTableViewIvars {
    delegate: RefCell<Weak<AppDelegate>>,
}

define_class!(
    // SAFETY: The Containers source-list table and its delegate are owned by the AppKit main
    // thread. Context menus are built synchronously for the row beneath the pointer.
    #[unsafe(super = NSTableView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ContainersTableViewIvars]
    struct ContainersTableView;

    unsafe impl NSObjectProtocol for ContainersTableView {}

    impl ContainersTableView {
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let row = self.rowAtPoint(point);
            if let (Ok(row), Some(delegate)) = (
                usize::try_from(row),
                self.ivars().delegate.borrow().load(),
            ) {
                delegate.prepare_container_menu_for_row(row)
            } else {
                None
            }
        }
    }
);

impl ContainersTableView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ContainersTableViewIvars {
            delegate: RefCell::new(Weak::default()),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn attach_delegate(&self, delegate: &AppDelegate) {
        self.ivars().delegate.replace(Weak::new(delegate));
    }
}

struct FilesUi {
    sidebar_root: Retained<NSView>,
    search: Retained<NSSearchField>,
    table: Retained<FilesTableView>,
    menu: Retained<NSMenu>,
    scroll: Retained<NSScrollView>,
    status: Retained<NSTextField>,
    spinner: Retained<NSProgressIndicator>,
    content_root: Retained<NSView>,
    title: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    metadata_base: RefCell<String>,
    empty: Retained<NSTextField>,
    preview_scroll: Retained<NSScrollView>,
    preview_text: Retained<NSTextView>,
    preview_code: Retained<CodeMetalView>,
    editor_search_panel: Retained<NSGlassEffectView>,
    editor_search: Retained<NSSearchField>,
    editor_search_status: Retained<NSTextField>,
    editor_search_visible: Cell<bool>,
    preview_web: Retained<WKWebView>,
    preview_web_content: Retained<WKUserContentController>,
    preview_divider: Retained<NSBox>,
    preview_image: Retained<NativeImagePreview>,
    preview_pdf: Retained<PDFView>,
    preview_table_scroll: Retained<NSScrollView>,
    preview_table: Retained<NSTableView>,
    preview_table_columns: RefCell<Vec<String>>,
    preview_table_rows: RefCell<Vec<Vec<String>>>,
    font_registration: RefCell<Option<NativeFontRegistration>>,
    sqlite_controls: Retained<NSView>,
    sqlite_table_selector: Retained<NSPopUpButton>,
    sqlite_column_selector: Retained<NSPopUpButton>,
    sqlite_filter: Retained<NSSearchField>,
    sqlite_previous: Retained<NSButton>,
    sqlite_next: Retained<NSButton>,
    sqlite_reload: Retained<NSButton>,
    sqlite_status: Retained<NSTextField>,
    sqlite_state: RefCell<Option<NativeSqliteState>>,
    sqlite_generation: Cell<u64>,
    preview_spinner: Retained<NSProgressIndicator>,
    rows: RefCell<Vec<NativeFileRow>>,
    expanded: RefCell<HashSet<FileNodePath>>,
    selected_path: RefCell<Option<FileNodePath>>,
    query: RefCell<String>,
    generation: Cell<u64>,
    loading: Cell<bool>,
    mutation_in_progress: Cell<bool>,
    dirty: Cell<bool>,
    preview_request_id: Cell<u64>,
    drop_hover_generation: Cell<u64>,
    drop_hover_path: RefCell<Option<FileNodePath>>,
    loaded_text_path: RefCell<Option<FileNodePath>>,
    loaded_text_signature: RefCell<Option<FileSignature>>,
    text_buffer: RefCell<String>,
    text_selection: Cell<NSRange>,
    text_editable: Cell<bool>,
    pending_text_selection: RefCell<Option<FileNodePath>>,
    text_edit_generation: Cell<u64>,
    text_dirty: Cell<bool>,
    text_save_in_progress: Cell<bool>,
    preview_web_mode: Cell<NativeWebPreviewMode>,
    markdown_editor_source_offset: Cell<Option<usize>>,
    suppress_text_change: Cell<bool>,
}

#[derive(Clone)]
struct NativeWorkspaceMetadata {
    kind: RepoMetadata,
    remote_label: Option<String>,
}

struct NativeWorkspaceMetadataRequest {
    generation: u64,
    entries: Vec<WorkspaceEntry>,
}

struct NativeWorkspaceSettings {
    settings: GitSettings,
    github_accounts: Result<Vec<GitHubAuthAccount>, String>,
}

struct NativeFontRegistration {
    descriptors: CFRetained<CFArray>,
}

struct NativeSqliteSchema {
    db_path: PathBuf,
    materialized: Option<MaterializedFile>,
    tables: Vec<sqlite_preview::Table>,
}

struct NativeSqliteState {
    db_path: PathBuf,
    _materialized: Option<MaterializedFile>,
    tables: Vec<sqlite_preview::Table>,
    selected_table: usize,
    columns: Vec<sqlite_preview::Column>,
    page: usize,
    total_rows: usize,
    filter: String,
    filter_column: Option<usize>,
    sort: Option<NativeSqliteSort>,
}

impl Drop for NativeFontRegistration {
    fn drop(&mut self) {
        // SAFETY: This CFArray was produced by Core Text and contains font descriptors.
        unsafe {
            CTFontManagerUnregisterFontDescriptors(
                &self.descriptors,
                CTFontManagerScope::Process,
                None,
            );
        }
    }
}

struct ContainersUi {
    sidebar_root: Retained<NSView>,
    search: Retained<NSSearchField>,
    table: Retained<ContainersTableView>,
    scroll: Retained<NSScrollView>,
    status: Retained<NSTextField>,
    spinner: Retained<NSProgressIndicator>,
    content_root: Retained<NSView>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    empty: Retained<NSTextField>,
    details_scroll: Retained<NSScrollView>,
    details_content: Retained<NSView>,
    inspect_code: Retained<CodeMetalView>,
    logs: Retained<NSButton>,
    inspect: Retained<NSButton>,
    shell: Retained<NSButton>,
    start: Retained<NSButton>,
    stop: Retained<NSButton>,
    restart: Retained<NSButton>,
    remove: Retained<NSButton>,
    menu: Retained<NSMenu>,
    rows: RefCell<Vec<NativeContainerRow>>,
    expanded_groups: RefCell<HashSet<String>>,
    selected_id: RefCell<Option<String>>,
    selected_group_key: RefCell<Option<String>>,
    query: RefCell<String>,
    generation: Cell<u64>,
    loading: Cell<bool>,
    dirty: Cell<bool>,
    detail_request_id: Cell<u64>,
    action_request_id: Cell<u64>,
    action_in_progress: Cell<bool>,
    context_selection: Cell<bool>,
}

struct AgentsUi {
    sidebar_root: Retained<NSView>,
    new_chat: Retained<NSButton>,
    codex_cli: Retained<NSButton>,
    agy: Retained<NSButton>,
    history_search: Retained<NSSearchField>,
    history_scope: Retained<NSPopUpButton>,
    threads_scroll: Retained<NSScrollView>,
    threads_document: Retained<NSView>,
    content_root: Retained<NSView>,
    title: Retained<NSTextField>,
    status: Retained<NSTextField>,
    spinner: Retained<NSProgressIndicator>,
    tools: Retained<NSPopUpButton>,
    thread_actions: Retained<NSPopUpButton>,
    model: Retained<NSPopUpButton>,
    reasoning: Retained<NSPopUpButton>,
    personality: Retained<NSPopUpButton>,
    service_tier: Retained<NSPopUpButton>,
    permissions: Retained<NSPopUpButton>,
    usage: Retained<NSTextField>,
    usage_progress: Retained<NSProgressIndicator>,
    transcript_scroll: Retained<NSScrollView>,
    transcript_table: Retained<NSTableView>,
    empty: Retained<NSTextField>,
    composer_scroll: Retained<NSScrollView>,
    composer: Retained<NSTextView>,
    attach: Retained<NSPopUpButton>,
    attachment_tokens: Retained<NSTokenField>,
    clear_attachments: Retained<NSButton>,
    send: Retained<NSButton>,
    stop: Retained<NSButton>,
    separator: Retained<NSBox>,
    terminal_panel: Retained<NSView>,
    terminal_stack: Retained<NSView>,
    terminal_cards: RefCell<HashMap<isize, NativeAgentTerminalCard>>,
    model_options: RefCell<Vec<NativeAgentSelectorOption>>,
    reasoning_options: RefCell<Vec<NativeAgentSelectorOption>>,
    personality_options: RefCell<Vec<NativeAgentSelectorOption>>,
    service_tier_options: RefCell<Vec<NativeAgentSelectorOption>>,
    permission_options: RefCell<Vec<NativeAgentSelectorOption>>,
    selected_model: RefCell<Option<String>>,
    selected_reasoning: RefCell<Option<String>>,
    selected_personality: RefCell<Option<String>>,
    selected_service_tier: RefCell<Option<String>>,
    selected_permissions: RefCell<Option<String>>,
    selector_updates_suppressed: Cell<bool>,
    threads: RefCell<Vec<NativeAgentThreadSummary>>,
    history_query: RefCell<String>,
    history_archived: Cell<bool>,
    active_thread_id: RefCell<Option<String>>,
    transcript_items: RefCell<Vec<NativeAgentTranscriptItem>>,
    transcript_images: RefCell<HashMap<String, NativeAgentTranscriptImage>>,
    transcript_image_order: RefCell<VecDeque<String>>,
    transcript_image_in_flight: RefCell<HashMap<String, NativeAgentTranscriptImageSource>>,
    transcript_image_errors: RefCell<HashMap<String, NativeAgentTranscriptImageError>>,
    attachments: RefCell<Vec<NativeAgentAttachment>>,
    generation: Cell<u64>,
    state: Cell<NativeAgentState>,
}

struct NativeAgentTerminalCard {
    container: Retained<NSBox>,
    icon: Retained<NSImageView>,
    selector: Retained<NSButton>,
    title: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    resource: Retained<NSTextField>,
}

struct NativeAgentTranscriptImage {
    source: NativeAgentTranscriptImageSource,
    image: Retained<NSImage>,
}

struct NativeAgentTranscriptImageError {
    source: NativeAgentTranscriptImageSource,
    message: String,
}

struct CommitMessageSettingsUi {
    window: Retained<NSWindow>,
    shell_font_size: Retained<NSTextField>,
    editor_font_size: Retained<NSTextField>,
    diff_font_size: Retained<NSTextField>,
    agent_font_size: Retained<NSTextField>,
    font_status: Retained<NSTextField>,
    provider: Retained<NSPopUpButton>,
    model: Retained<NSPopUpButton>,
    spinner: Retained<NSProgressIndicator>,
    status: Retained<NSTextField>,
    provider_ids: Vec<String>,
    model_ids: RefCell<Vec<Option<String>>>,
    current_provider: RefCell<String>,
    request_id: Cell<u64>,
    workspace_section: Retained<NSView>,
    workspace_spinner: Retained<NSProgressIndicator>,
    workspace_status: Retained<NSTextField>,
    use_global_user: Retained<NSButton>,
    author_name: Retained<NSTextField>,
    author_email: Retained<NSTextField>,
    commit_timezone: Retained<NSTextField>,
    use_system_timezone: Retained<NSButton>,
    remote_owner_warning: Retained<NSButton>,
    github_account: Retained<NSPopUpButton>,
    github_accounts: RefCell<Vec<Option<GitHubAuthAccount>>>,
    workspace_settings: RefCell<Option<GitSettings>>,
    save_workspace: Retained<NSButton>,
    workspace_loading: Cell<bool>,
    workspace_request_id: Cell<u64>,
}

#[derive(Clone)]
struct NativeFileRow {
    info: FileNodeInfo,
    depth: usize,
}

struct NativeTextAnalysis {
    syntax: Vec<TextSyntaxSpan>,
    diagnostics: Vec<TextDiagnosticSpan>,
    fold_ranges: Vec<(usize, usize)>,
    markdown_lint: Vec<MarkdownLintIssue>,
    completion: Option<CompletionSet>,
    completion_cursor_utf16: Option<usize>,
    web_preview: Option<Result<NativeWebPreview, String>>,
    csv_table: Option<Result<Option<CsvTable>, String>>,
}

struct NativeFolderPreview {
    info: FileNodeInfo,
    provider_path: String,
    file_count: usize,
    folder_count: usize,
}

struct NativeWebPreview {
    html: String,
    mode: NativeWebPreviewMode,
}

#[derive(Clone)]
enum NativeContainerRow {
    Group(ContainerGroup),
    Container(ContainerSummary),
}

#[derive(Clone)]
enum NativeFileMutation {
    CreateFile {
        path: FileNodePath,
    },
    CreateDirectory {
        path: FileNodePath,
    },
    Rename {
        source: FileNodePath,
        destination_parent: FileNodePath,
        new_name: String,
    },
    Copy {
        source: FileNodePath,
        destination: FileNodePath,
    },
    Move {
        source: FileNodePath,
        destination_parent: FileNodePath,
        new_name: String,
    },
    Transfer {
        source_workspace: craic_config::ConfiguredWorkspace,
        source_workspace_id: String,
        source_relative: String,
        destination: FileNodePath,
    },
    Upload {
        sources: Vec<PathBuf>,
        destination_parent: FileNodePath,
    },
    Delete {
        path: FileNodePath,
    },
}

#[derive(Clone)]
enum NativeSudoRetry {
    Mutation(NativeFileMutation),
    Download {
        source: FileNodePath,
        destination: FileDownloadDestination,
    },
    Save {
        path: FileNodePath,
        text: String,
        expected_signature: FileSignature,
        edit_generation: u64,
    },
}

impl NativeFileMutation {
    fn progress_label(&self) -> &'static str {
        match self {
            Self::CreateFile { .. } => "Creating file…",
            Self::CreateDirectory { .. } => "Creating folder…",
            Self::Rename { .. } => "Renaming item…",
            Self::Copy { .. } => "Copying item…",
            Self::Move { .. } => "Moving item…",
            Self::Transfer { .. } => "Copying item…",
            Self::Upload { .. } => "Uploading items…",
            Self::Delete { .. } => "Deleting item…",
        }
    }
}

struct NativeFileMonitor {
    subscription: Option<FileWatchSubscription>,
    bridge: Option<thread::JoinHandle<()>>,
}

impl NativeFileMonitor {
    fn new(
        subscription: FileWatchSubscription,
        receiver: FileWatchReceiver,
        workspace_id: String,
        completions: std::sync::mpsc::Sender<FrontendCompletion>,
    ) -> Self {
        let bridge = thread::spawn(move || {
            let mut receiver = receiver;
            while receiver.blocking_recv().is_some() {
                let _ = completions.send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceFilesChanged {
                        workspace_id: workspace_id.clone(),
                    },
                ));
            }
            log::debug!("native Files watch bridge stopped workspace={workspace_id}");
        });
        Self {
            subscription: Some(subscription),
            bridge: Some(bridge),
        }
    }
}

impl Drop for NativeFileMonitor {
    fn drop(&mut self) {
        self.subscription.take();
        if let Some(bridge) = self.bridge.take()
            && bridge.join().is_err()
        {
            log::warn!("native Files watch bridge panicked during shutdown");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeTerminalPlacement {
    General,
    Agent,
}

struct NativeTerminalSession {
    id: isize,
    tab: Retained<NSView>,
    title: Retained<NSButton>,
    title_label: String,
    agent_provider_label: Option<String>,
    view: Retained<TerminalMetalView>,
    working_directory: String,
    local_working_directory: Option<PathBuf>,
    placement: NativeTerminalPlacement,
    activity: ShellCommandActivity,
    reported_task_active: bool,
    auto_close_timer: Option<Retained<NSTimer>>,
    remote_media: Option<NativeTerminalRemoteMedia>,
}

#[derive(Clone)]
struct NativeTerminalRemoteMedia {
    workspace_id: String,
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    cancellation: WorkspaceCancellationToken,
}

enum NativeTerminalMediaCommand {
    Upload {
        session_id: isize,
        context: NativeTerminalRemoteMedia,
        sources: Vec<PathBuf>,
    },
    Close {
        session_id: isize,
    },
    Shutdown,
}

struct NativeTerminalMediaWorkerSession {
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    uploaded: Vec<RemoteMedia>,
}

impl NativeTerminalSession {
    fn has_active_task(&self) -> bool {
        if !self.view.is_active() {
            return false;
        }
        match self.activity {
            ShellCommandActivity::Command | ShellCommandActivity::LocalInteractiveShell => true,
            ShellCommandActivity::LogStream => false,
            ShellCommandActivity::ReportedInteractiveShell => self.reported_task_active,
        }
    }
}

#[derive(Default)]
pub(crate) struct AppDelegateIvars {
    font_sizes: Cell<craic_config::FontSizes>,
    window: OnceCell<Retained<NSWindow>>,
    app_handle: OnceCell<AppHandle>,
    ui_context: OnceCell<UiContextId>,
    frontend_completions: OnceCell<std::sync::mpsc::Sender<FrontendCompletion>>,
    frontend_requests: OnceCell<tokio::sync::mpsc::Sender<FrontendRequest>>,
    startup_error: RefCell<Option<String>>,
    workspace_button: OnceCell<Retained<NSButton>>,
    workspace_button_spinner: OnceCell<Retained<NSProgressIndicator>>,
    workspace_popover: OnceCell<Retained<NSPopover>>,
    workspace_search: OnceCell<Retained<NSSearchField>>,
    workspace_table: OnceCell<Retained<NSTableView>>,
    workspace_add_button: OnceCell<Retained<NSButton>>,
    workspace_results: RefCell<Vec<usize>>,
    workspaces: RefCell<Vec<WorkspaceEntry>>,
    workspace_discovery_loading: Cell<bool>,
    workspace_discovery_generation: Cell<u64>,
    workspace_discovery_requests: OnceCell<tokio::sync::mpsc::Sender<WorkspaceDiscoveryRequest>>,
    workspace_create_request_id: Cell<u64>,
    workspace_create_in_progress: Cell<bool>,
    workspace_create_name: RefCell<Option<Retained<NSTextField>>>,
    workspace_create_remote: RefCell<Option<Retained<NSTextField>>>,
    workspace_create_root: RefCell<Option<Retained<NSPopUpButton>>>,
    workspace_create_roots: RefCell<Vec<PathBuf>>,
    workspace_create_button: RefCell<Option<Retained<NSButton>>>,
    workspace_create_cancel_button: RefCell<Option<Retained<NSButton>>>,
    workspace_create_has_root: Cell<bool>,
    workspace_create_auto_name: Cell<bool>,
    workspace_create_updating_name: Cell<bool>,
    workspace_create_form: RefCell<Option<Retained<NSAlert>>>,
    workspace_create_spinner: RefCell<Option<Retained<NSProgressIndicator>>>,
    workspace_create_status: RefCell<Option<Retained<NSTextField>>>,
    workspace_create_pending_success: RefCell<Option<(PathBuf, String)>>,
    workspace_metadata: RefCell<HashMap<String, NativeWorkspaceMetadata>>,
    workspace_metadata_pending: RefCell<HashSet<String>>,
    workspace_metadata_generation: Cell<u64>,
    workspace_metadata_requests:
        OnceCell<tokio::sync::mpsc::Sender<NativeWorkspaceMetadataRequest>>,
    active_workspace_id: RefCell<Option<String>>,
    workspace_generation: Cell<craic_app_core::Generation>,
    workspace_refresh_loading: Cell<bool>,
    repository_requests: OnceCell<tokio::sync::mpsc::Sender<RepositoryRequest>>,
    agent_commands: OnceCell<tokio::sync::mpsc::Sender<NativeAgentCommand>>,
    agent_request_alerts: RefCell<HashMap<String, Retained<NSAlert>>>,
    agent_pending_request_keys: RefCell<HashSet<String>>,
    agent_request_multiline_inputs:
        RefCell<HashMap<String, (Retained<NSTextView>, Retained<NSButton>)>>,
    close_confirmation: RefCell<Option<Retained<NSAlert>>>,
    shortcuts_window: RefCell<Option<Retained<NSWindow>>>,
    quit_requested_during_close_confirmation: Cell<bool>,
    close_confirmed: Cell<bool>,
    shutdown_prepared: Cell<bool>,
    repository_monitor: RefCell<Option<ChangeListenerSubscription>>,
    repository_background_pull: RefCell<Option<BackgroundPullSubscription>>,
    files_monitor: RefCell<Option<NativeFileMonitor>>,
    workspace_handle: RefCell<Option<Arc<GitRepoHandle>>>,
    git_handle: RefCell<Option<Arc<GitRepoHandle>>>,
    repository_snapshot: RefCell<Option<RepositorySnapshot>>,
    avatar_images: RefCell<HashMap<String, Retained<NSImage>>>,
    avatar_in_flight: RefCell<HashSet<String>>,
    commit_avatar_source: RefCell<Option<String>>,
    repository_loading: Cell<bool>,
    repository_initialization_in_progress: Cell<bool>,
    branch_button: OnceCell<Retained<NSButton>>,
    branch_popover: OnceCell<Retained<NSPopover>>,
    branch_search: OnceCell<Retained<NSSearchField>>,
    branch_list: OnceCell<Retained<NSView>>,
    branch_footer: OnceCell<Retained<NSButton>>,
    branch_merge_mode: Cell<bool>,
    author_popover: OnceCell<Retained<NSPopover>>,
    author_table: OnceCell<Retained<NSTableView>>,
    author_options: RefCell<Vec<CommitEmailOption>>,
    author_loading: Cell<bool>,
    author_selection_suppressed: Cell<bool>,
    author_error: RefCell<Option<String>>,
    author_warning_popover: RefCell<Option<Retained<NSPopover>>>,
    fetch_button: OnceCell<Retained<NSButton>>,
    fetch_spinner: OnceCell<Retained<NSProgressIndicator>>,
    quick_action_group: OnceCell<Retained<NSToolbarItemGroup>>,
    quick_action_items: RefCell<Vec<Retained<NSMenuToolbarItem>>>,
    quick_action_targets: RefCell<Vec<RunItem>>,
    quick_action_configs: RefCell<Vec<QuickActionConfig>>,
    quick_action_workspace_id: RefCell<Option<String>>,
    quick_action_repo_path: RefCell<Option<PathBuf>>,
    quick_action_generation: Cell<u64>,
    quick_action_loading: Cell<bool>,
    terminal_toolbar_item: OnceCell<Retained<NSToolbarItem>>,
    split_controller: OnceCell<Retained<NSSplitViewController>>,
    sidebar: OnceCell<Retained<NSView>>,
    changes_split: OnceCell<Retained<NSSplitView>>,
    changes_browser: OnceCell<Retained<NSView>>,
    content: OnceCell<Retained<NSView>>,
    toast: OnceCell<Retained<NSGlassEffectView>>,
    toast_label: OnceCell<Retained<NSTextField>>,
    toast_timer: RefCell<Option<Retained<NSTimer>>>,
    content_split: OnceCell<Retained<NSSplitView>>,
    terminal_panel: OnceCell<Retained<NSView>>,
    terminal_tab_strip: OnceCell<Retained<NSView>>,
    terminal_stack: OnceCell<Retained<NSView>>,
    terminal_search_panel: OnceCell<Retained<NSView>>,
    terminal_search: OnceCell<Retained<NSSearchField>>,
    terminal_search_status: OnceCell<Retained<NSTextField>>,
    terminal_search_case: OnceCell<Retained<NSButton>>,
    terminal_search_word: OnceCell<Retained<NSButton>>,
    terminal_search_regex: OnceCell<Retained<NSButton>>,
    terminal_search_visible: Cell<bool>,
    terminal_search_placement: Cell<Option<NativeTerminalPlacement>>,
    terminal_sessions: RefCell<Vec<NativeTerminalSession>>,
    terminal_media_commands: OnceCell<std::sync::mpsc::Sender<NativeTerminalMediaCommand>>,
    agent_terminal_usage_timer: RefCell<Option<Retained<NSTimer>>>,
    agent_terminal_usage_tracker: RefCell<Option<ProcessUsageTracker>>,
    agent_terminal_usage: RefCell<HashMap<isize, AgentResourceUsage>>,
    active_terminal_id: Cell<Option<isize>>,
    active_general_terminal_id: Cell<Option<isize>>,
    active_agent_terminal_id: Cell<Option<isize>>,
    next_terminal_id: Cell<isize>,
    terminal_visible: Cell<bool>,
    page_switcher: OnceCell<Retained<NSToolbarItemGroup>>,
    changes_edge_accessory: OnceCell<Retained<AnyObject>>,
    changes_edge_container: OnceCell<Retained<NSView>>,
    changes_edge_height: OnceCell<Retained<NSLayoutConstraint>>,
    changes_top_cover: OnceCell<Retained<NSGlassEffectView>>,
    changes_search_popup: OnceCell<Retained<NSGlassEffectView>>,
    active_page_id: RefCell<Option<String>>,
    page_state_revisions: RefCell<HashMap<String, u64>>,
    page_service_requests: RefCell<HashMap<String, PageServiceRequest>>,
    history: OnceCell<HistoryUi>,
    files: OnceCell<FilesUi>,
    containers: OnceCell<ContainersUi>,
    agents: OnceCell<AgentsUi>,
    commit_composer: OnceCell<CommitComposer>,
    commit_message_settings: OnceCell<CommitMessageSettingsUi>,
    commit_message_generation_id: Cell<u64>,
    commit_message_cancellation: RefCell<Option<CancellationToken>>,
    selection_header: OnceCell<Retained<NSView>>,
    select_all_check: OnceCell<Retained<NSButton>>,
    select_all_label: OnceCell<Retained<NSTextField>>,
    changes_search_panel: OnceCell<Retained<NSView>>,
    changes_search: OnceCell<Retained<NSSearchField>>,
    changes_search_visible: Cell<bool>,
    history_search_visible: Cell<bool>,
    files_search_visible: Cell<bool>,
    containers_search_visible: Cell<bool>,
    agents_search_visible: Cell<bool>,
    changes_filter_query: RefCell<String>,
    content_empty: OnceCell<Retained<NSTextField>>,
    content_home_root: OnceCell<Retained<NSView>>,
    content_home_title: OnceCell<Retained<NSTextField>>,
    content_home_subtitle: OnceCell<Retained<NSTextField>>,
    content_home_cards: OnceCell<Vec<Retained<NSBox>>>,
    content_home_git_title: OnceCell<Retained<NSTextField>>,
    content_home_git_subtitle: OnceCell<Retained<NSTextField>>,
    content_home_action: OnceCell<Retained<NSButton>>,
    content_home_initialize_card: OnceCell<Retained<NSBox>>,
    content_home_initialize: OnceCell<Retained<NSButton>>,
    content_home_editor: OnceCell<Retained<NSButton>>,
    content_home_terminal: OnceCell<Retained<NSButton>>,
    content_home_files: OnceCell<Retained<NSButton>>,
    content_home_remote: OnceCell<Retained<NSButton>>,
    changes_list: OnceCell<Retained<NSView>>,
    changes_scroll: OnceCell<Retained<NSScrollView>>,
    selected_change_path: RefCell<Option<String>>,
    pending_files_path: RefCell<Option<String>>,
    pending_files_line: Cell<Option<usize>>,
    pending_files_column: Cell<Option<usize>>,
    checked_change_paths: RefCell<HashSet<String>>,
    diff_request_id: Cell<u64>,
    diff_loading_request_id: Cell<Option<u64>>,
    loaded_diff_path: RefCell<Option<String>>,
    loaded_image_path: RefCell<Option<String>>,
    file_preview_cache: RefCell<VecDeque<CachedFilePreview>>,
    diff_view: OnceCell<Retained<DiffMetalView>>,
    image_preview: OnceCell<Retained<NSImageView>>,
    binary_preview: OnceCell<Retained<NSView>>,
    binary_font_registrations: RefCell<Vec<NativeFontRegistration>>,
    diff_search_panel: OnceCell<Retained<NSGlassEffectView>>,
    diff_search: OnceCell<Retained<NSSearchField>>,
    diff_search_status: OnceCell<Retained<NSTextField>>,
    diff_spinner: OnceCell<Retained<NSProgressIndicator>>,
}
