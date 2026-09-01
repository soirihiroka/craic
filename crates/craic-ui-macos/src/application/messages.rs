enum AvatarSource {
    Email(String),
    Url(String),
}
enum RepositoryRequest {
    Load {
        workspace: craic_config::ConfiguredWorkspace,
        cancellation: WorkspaceCancellationToken,
    },
    Refresh {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        core_request: Option<RepositoryCoreRefreshRequest>,
        cancellation: WorkspaceCancellationToken,
    },
    RunGitAction {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        snapshot: RepositorySnapshot,
        action: NativeRemoteAction,
        stash_before: bool,
        cancellation: WorkspaceCancellationToken,
    },
    LoadQuickActions {
        workspace_id: String,
        repo_path: PathBuf,
        generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    SaveQuickActionConfiguration {
        workspace_id: String,
        repo_path: PathBuf,
        configs: Vec<QuickActionConfig>,
    },
    RunBranchAction {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        action: BranchAction,
        cancellation: WorkspaceCancellationToken,
    },
    LoadFileComparison {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: String,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadFileBytesComparison {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: String,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadHistoryPage {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        query: String,
        after: Option<String>,
        generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadHistoryCommit {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        hash: String,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    RunHistoryAction {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        action: HistoryAction,
        cancellation: WorkspaceCancellationToken,
    },
    InitializeRepository {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        cancellation: WorkspaceCancellationToken,
    },
    LoadHistoryComparison {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        hash: String,
        path: String,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadHistoryBytesComparison {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        hash: String,
        path: String,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadFilesTree {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        expanded: HashSet<FileNodePath>,
        generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadWorkspaceFile {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: FileNodePath,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadWorkspaceFolder {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: FileNodePath,
        info: FileNodeInfo,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadWorkspaceSqliteSchema {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: FileNodePath,
        info: FileNodeInfo,
        prefetched_bytes: Option<Vec<u8>>,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadWorkspaceSqlitePage {
        workspace_id: String,
        path: FileNodePath,
        db_path: PathBuf,
        table: sqlite_preview::Table,
        page: usize,
        filter_column: Option<usize>,
        filter: String,
        sort: Option<NativeSqliteSort>,
        generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    RunFileMutation {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        mutation: NativeFileMutation,
        allow_sudo_retry: bool,
        cancellation: WorkspaceCancellationToken,
    },
    DownloadWorkspaceFile {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        source: FileNodePath,
        destination: FileDownloadDestination,
        allow_sudo_retry: bool,
        cancellation: WorkspaceCancellationToken,
    },
    AuthorizeFileSudo {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        password: Option<FileSudoPassword>,
        retry: NativeSudoRetry,
    },
    SaveWorkspaceFile {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        path: FileNodePath,
        text: String,
        expected_signature: FileSignature,
        edit_generation: u64,
        allow_sudo_retry: bool,
        cancellation: WorkspaceCancellationToken,
    },
    HighlightWorkspaceText {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        path: FileNodePath,
        text: String,
        completion_cursor: Option<usize>,
        completion_cursor_utf16: Option<usize>,
        edit_generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadContainers {
        workspace_id: String,
        access: Arc<dyn DockerAccess>,
        generation: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadContainerDetail {
        workspace_id: String,
        access: Arc<dyn DockerAccess>,
        container_id: String,
        request_id: u64,
        kind: ContainerDetailKind,
        cancellation: WorkspaceCancellationToken,
    },
    RunContainerAction {
        workspace_id: String,
        workspace_generation: craic_app_core::Generation,
        access: Arc<dyn DockerAccess>,
        container_id: String,
        action: docker::ContainerAction,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    RunComposeAction {
        workspace_id: String,
        workspace_generation: craic_app_core::Generation,
        access: Arc<dyn DockerAccess>,
        compose: ComposeProject,
        action: docker::ComposeAction,
        request_id: u64,
        cancellation: WorkspaceCancellationToken,
    },
    LoadAvatar {
        cache_key: String,
        source: AvatarSource,
        handle: Arc<GitRepoHandle>,
    },
    LoadCommitAuthors {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
    },
    ResolveAgentFileLink {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    LoadAgentImage {
        workspace_id: String,
        generation: u64,
        item_id: String,
        source: NativeAgentTranscriptImageSource,
        access: Arc<dyn FileAccess>,
        cancellation: WorkspaceCancellationToken,
    },
    SaveCommitAuthor {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        option: CommitEmailOption,
        cancellation: WorkspaceCancellationToken,
    },
    Commit {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        summary: String,
        description: String,
        files: Vec<String>,
        cancellation: WorkspaceCancellationToken,
    },
    GenerateCommitMessage {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        files: Vec<String>,
        request_id: u64,
        cancellation: CancellationToken,
        workspace_cancellation: WorkspaceCancellationToken,
    },
    LoadCommitMessageSettings {
        request_id: u64,
    },
    LoadCommitMessageModels {
        provider_id: String,
        selected_model: Option<String>,
        request_id: u64,
    },
    SaveCommitMessageProvider {
        provider_id: String,
    },
    SaveCommitMessageModel {
        provider_id: String,
        model: Option<String>,
    },
    LoadWorkspaceSettings {
        workspace_id: String,
        request_id: u64,
        workspace: craic_config::ConfiguredWorkspace,
        handle: Arc<GitRepoHandle>,
        cancellation: WorkspaceCancellationToken,
    },
    SaveWorkspaceSettings {
        workspace_id: String,
        request_id: u64,
        handle: Arc<GitRepoHandle>,
        settings: GitSettings,
        cancellation: WorkspaceCancellationToken,
    },
    Discard {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        paths: Vec<String>,
        cancellation: WorkspaceCancellationToken,
    },
    Stash {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        cancellation: WorkspaceCancellationToken,
    },
    AddIgnorePattern {
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        pattern: String,
        cancellation: WorkspaceCancellationToken,
    },
}

enum HistoryAction {
    Checkout {
        hash: String,
        parent: bool,
    },
    CreateBranch {
        branch: String,
        hash: String,
    },
    CreateTag {
        tag: String,
        hash: String,
    },
    CherryPick(String),
    Revert(String),
    Amend {
        summary: String,
        description: String,
    },
    Reset {
        hash: String,
        mode: ResetMode,
    },
}

#[derive(Clone)]
enum RepositoryCoreRefreshRequest {
    Page(String),
    Workspace(WorkspaceRefreshRequest),
}

struct NativeQuickActions {
    targets: Vec<RunItem>,
    configs: Vec<QuickActionConfig>,
}

#[derive(Clone)]
struct PreparedDiff {
    fingerprint: u64,
    source_rows: usize,
    document: DiffDocument,
    syntax: Vec<DiffSyntaxSpan>,
}

#[derive(Clone)]
enum CachedFilePreviewContent {
    Diff(PreparedDiff),
    Image(BytesComparison),
    Unavailable(String),
}

#[derive(Clone)]
struct CachedFilePreview {
    path: String,
    content: CachedFilePreviewContent,
}

#[derive(Clone)]
enum CachedHistoryPreviewContent {
    Diff(PreparedDiff),
    Binary(BytesComparison),
    Unavailable(String),
}

#[derive(Clone)]
struct CachedHistoryPreview {
    hash: String,
    path: String,
    content: CachedHistoryPreviewContent,
}

fn prepare_diff(path: &str, comparison: FileComparison) -> PreparedDiff {
    let fingerprint = comparison.fingerprint;
    let source_rows = comparison.rows.len();
    let document = diff_document(&comparison);
    let syntax = build_diff_syntax(path, &document.rows);
    PreparedDiff {
        fingerprint,
        source_rows,
        document,
        syntax,
    }
}

enum FrontendCompletion {
    Repository(RepositoryCompletion),
    Agent(NativeAgentEvent),
    WorkspaceEntries {
        generation: u64,
        entries: Vec<WorkspaceEntry>,
        preferred: Option<craic_config::ConfiguredWorkspace>,
        select_workspace: bool,
    },
    WorkspaceDiscoveryFailed {
        generation: u64,
        message: String,
    },
    WorkspaceCreated {
        request_id: u64,
        result: Result<(PathBuf, String), String>,
    },
    WorkspaceMetadata {
        workspace_id: String,
        generation: u64,
        result: Result<NativeWorkspaceMetadata, String>,
    },
    OpenWorkspace(UiEffectResult),
    ConfirmDiscard {
        paths: Vec<String>,
        result: UiEffectResult,
    },
    TerminalRemoteImages {
        workspace_id: String,
        session_id: isize,
        result: Result<Vec<String>, String>,
    },
    Shutdown,
}

enum FrontendRequest {
    OpenWorkspace,
    CreateWorkspace {
        request_id: u64,
        request: NativeCreateWorkspaceRequest,
    },
    ConfirmDiscard {
        paths: Vec<String>,
        heading: String,
        message: String,
    },
    SaveLastWorkspace(craic_config::ConfiguredWorkspace),
}

struct WorkspaceDiscoveryRequest {
    generation: u64,
    preferred: Option<craic_config::ConfiguredWorkspace>,
    select_workspace: bool,
}

struct NativeCreateWorkspaceRequest {
    root: PathBuf,
    name: String,
    remote: Option<String>,
}

enum RepositoryCompletion {
    Snapshot {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Option<Arc<GitRepoHandle>>,
        core_request: Option<RepositoryCoreRefreshRequest>,
        result: Result<WorkspaceSnapshot, String>,
    },
    ActionProgress {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        message: String,
    },
    ActionFailed {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        title: &'static str,
        message: String,
    },
    ActionNeedsStash {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        snapshot: RepositorySnapshot,
        action: NativeRemoteAction,
        files: Vec<String>,
    },
    ActionFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
        message: Option<String>,
    },
    QuickActions {
        workspace_id: String,
        generation: u64,
        result: Result<NativeQuickActions, String>,
    },
    QuickActionConfigurationSaved {
        workspace_id: String,
        result: Result<(), String>,
    },
    BranchProgress {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        message: String,
    },
    BranchFailed {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        message: String,
    },
    BranchFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
        message: String,
    },
    FileComparison {
        workspace_id: String,
        path: String,
        request_id: u64,
        result: Result<PreparedDiff, String>,
    },
    FileBytesComparison {
        workspace_id: String,
        path: String,
        request_id: u64,
        result: Result<BytesComparison, String>,
    },
    HistoryPage {
        workspace_id: String,
        generation: u64,
        result: Result<CommitPage, String>,
    },
    HistoryCommit {
        workspace_id: String,
        hash: String,
        request_id: u64,
        result: Result<(Commit, Vec<ChangedFile>, Option<String>, bool), String>,
    },
    HistoryActionProgress {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        message: String,
    },
    HistoryActionFailed {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        title: &'static str,
        message: String,
    },
    HistoryActionFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
        message: String,
    },
    RepositoryInitializationFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
    },
    RepositoryInitializationFailed {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        message: String,
    },
    HistoryComparison {
        workspace_id: String,
        hash: String,
        path: String,
        request_id: u64,
        result: Result<PreparedDiff, String>,
    },
    HistoryBytesComparison {
        workspace_id: String,
        hash: String,
        path: String,
        request_id: u64,
        result: Result<BytesComparison, String>,
    },
    FilesTree {
        workspace_id: String,
        generation: u64,
        result: Result<Vec<NativeFileRow>, String>,
    },
    WorkspaceFile {
        workspace_id: String,
        path: FileNodePath,
        request_id: u64,
        result: Result<FileRead, String>,
    },
    WorkspaceFolder {
        workspace_id: String,
        path: FileNodePath,
        request_id: u64,
        result: Result<NativeFolderPreview, String>,
    },
    WorkspaceSqliteSchema {
        workspace_id: String,
        path: FileNodePath,
        request_id: u64,
        result: Result<NativeSqliteSchema, String>,
    },
    WorkspaceSqlitePage {
        workspace_id: String,
        path: FileNodePath,
        generation: u64,
        result: Result<sqlite_preview::Page, String>,
    },
    WorkspaceFilesChanged {
        workspace_id: String,
    },
    FileMutationFinished {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        mutation: NativeFileMutation,
        allow_sudo_retry: bool,
        result: Result<Option<FileNodePath>, String>,
    },
    FileDownloadFinished {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        source: FileNodePath,
        destination: FileDownloadDestination,
        allow_sudo_retry: bool,
        result: Result<Vec<PathBuf>, String>,
    },
    FileSudoAuthorized {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        retry: NativeSudoRetry,
        result: Result<Arc<dyn FileAccess>, FileSudoError>,
    },
    WorkspaceFileSaved {
        workspace_id: String,
        access: Arc<dyn FileAccess>,
        path: FileNodePath,
        text: String,
        expected_signature: FileSignature,
        edit_generation: u64,
        allow_sudo_retry: bool,
        result: Result<FileNodeInfo, String>,
    },
    WorkspaceTextHighlighted {
        workspace_id: String,
        path: FileNodePath,
        edit_generation: u64,
        result: Result<NativeTextAnalysis, String>,
    },
    Containers {
        workspace_id: String,
        generation: u64,
        result: Result<ContainerInventory, String>,
    },
    ContainerDetail {
        workspace_id: String,
        container_id: String,
        request_id: u64,
        kind: ContainerDetailKind,
        result: Result<String, String>,
    },
    ContainerActionFinished {
        workspace_id: String,
        workspace_generation: craic_app_core::Generation,
        request_id: u64,
        result: Result<String, String>,
    },
    Avatar {
        cache_key: String,
        result: Result<Vec<u8>, String>,
    },
    CommitAuthors {
        workspace_id: String,
        result: Result<Vec<CommitEmailOption>, String>,
    },
    AgentFileLinkResolved {
        workspace_id: String,
        line: Option<usize>,
        column: Option<usize>,
        result: Result<TerminalLinkTarget, String>,
    },
    AgentImage {
        workspace_id: String,
        generation: u64,
        item_id: String,
        source: NativeAgentTranscriptImageSource,
        result: Result<Vec<u8>, String>,
    },
    CommitAuthorFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
    },
    CommitFinished {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        handle: Option<Arc<GitRepoHandle>>,
        snapshot: Option<Result<WorkspaceSnapshot, String>>,
        result: Result<String, String>,
    },
    CommitMessageGenerated {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        request_id: u64,
        provider_label: String,
        result: Result<CommitMessageDraft, String>,
    },
    CommitMessageSettingsLoaded {
        request_id: u64,
        provider_id: String,
        model: Option<String>,
    },
    CommitMessageModelsLoaded {
        request_id: u64,
        provider_id: String,
        selected_model: Option<String>,
        result: Result<Vec<ModelOption>, String>,
    },
    CommitMessageSettingsFailed {
        message: String,
    },
    WorkspaceSettingsLoaded {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        request_id: u64,
        result: Result<NativeWorkspaceSettings, String>,
    },
    WorkspaceSettingsSaved {
        workspace_id: String,
        cancellation: WorkspaceCancellationToken,
        request_id: u64,
        handle: Arc<GitRepoHandle>,
        result: Result<WorkspaceSnapshot, String>,
    },
    ChangesFailed {
        cancellation: WorkspaceCancellationToken,
        title: &'static str,
        message: String,
    },
}

enum BranchAction {
    Checkout(String),
    Create(String),
    Merge(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContainerDetailKind {
    Inspect,
}
