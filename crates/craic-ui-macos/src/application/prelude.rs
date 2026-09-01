use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use crate::agent_session::{
    ActiveThreadAction as NativeAgentThreadAction, Attachment as NativeAgentAttachment,
    AttachmentKind as NativeAgentAttachmentKind,
    BackgroundTerminal as NativeAgentBackgroundTerminal, Command as NativeAgentCommand,
    DEFAULT_SERVICE_TIER_ID, Event as NativeAgentEvent,
    ExperimentalFeature as NativeAgentExperimentalFeature,
    PendingRequest as NativeAgentPendingRequest, RequestResponse as NativeAgentRequestResponse,
    ReviewTarget as NativeAgentReviewTarget, SelectorOption as NativeAgentSelectorOption,
    SessionIdentity as AgentIdentity, SessionState as NativeAgentState,
    SkillOption as NativeAgentSkillOption, ThreadSummary as NativeAgentThreadSummary,
    TokenUsage as NativeAgentTokenUsage, ToolAction as NativeAgentToolAction,
    TranscriptImageSource as NativeAgentTranscriptImageSource,
    TranscriptItem as NativeAgentTranscriptItem, TranscriptKind as NativeAgentTranscriptKind,
    TranscriptStatus as NativeAgentTranscriptStatus,
};
use crate::code_view::CodeMetalView;
use crate::commit_composer::{COMMIT_COMPOSER_HEIGHT, CommitComposer, CommitComposerActions};
use crate::diff_view::DiffMetalView;
use crate::image_view::NativeImagePreview;
use crate::sqlite_preview::{
    self, Sort as NativeSqliteSort, SortDirection as NativeSqliteSortDirection,
};
use crate::terminal_view::TerminalMetalView;
use base64::Engine;
use block2::RcBlock;
use craic_agent::agent_provider::{
    CancellationToken, ModelOption, default_provider, find_provider, is_canceled_error,
    model_options, registered_providers,
};
use craic_agent::agent_usage::{AgentResourceUsage, ProcessSnapshot, ProcessUsageTracker};
use craic_agent::ai_commit::CommitMessageDraft;
use craic_agent::display::permission_profile_label;
use craic_agent::remote_media::{self, RemoteMedia, RemoteMediaKind};
use craic_app_core::{
    ActionId, AppCommand, AppHandle, ApplicationRuntime, ApplicationViewState, Badge,
    PAGE_DESCRIPTORS, PageCommand, PageId, PageServiceRequest, PageViewState, RefreshScope,
    RetiredJobSender, RuntimeConfig, ServiceCompletion, UiEvent, WorkspaceId,
    WorkspaceRefreshCompletion, WorkspaceRefreshIdentity, WorkspaceRefreshRequest,
    WorkspaceSelection, page_descriptor,
};
use craic_file_support::{
    ContentKind, FileProbe, FileRole, LanguageId, resolve as resolve_file_support,
};
use craic_language::markdown_lint::MarkdownLintIssue;
use craic_language::{
    CompletionSet, LinkTarget, LintKind, destination_target, detected_links, language_id_from_path,
    language_support_for_id,
};
use craic_platform::{
    ConfirmRequest, MainThreadDispatcher, PathPickerMode, PathPickerRequest, UiContextId, UiEffect,
    UiEffectCompletion, UiEffectId, UiEffectResult,
};
use craic_project::quick_action::{self, RunCommand, RunItem};
use craic_project::workspace_config::{self, QuickActionConfig};
use craic_render_skia::{
    DiffDocument, DiffRow, DiffRowKind, DiffSyntaxSpan, TerminalSearchDirection,
    TextDiagnosticKind, TextDiagnosticSpan, TextSyntaxAnalysis, TextSyntaxSpan,
    analyze_text_syntax, build_diff_syntax,
};
use craic_system::SystemProvider;
use craic_system::system::capabilities::docker::{ComposeFileAction, DockerAccess};
use craic_system::system::capabilities::files::{
    FileAccess, FileCopyRequest, FileDeleteRequest, FileDownloadDestination, FileDownloadRequest,
    FileMoveRequest, FileNodeInfo, FileNodeKind, FileOperationEvent, FileOperationReceiver,
    FileRead, FileReadRequest, FileSignature, FileSudoError, FileSudoErrorKind, FileSudoPassword,
    FileWatchReceiver, FileWatchRequest, FileWatchSubscription, FileWriteMode, FileWritePayload,
    FileWriteRequest,
};
use craic_system::system::capabilities::shell::{
    ShellAccess, ShellCommandActivity, ShellCommandSpec,
};
use craic_system::system::capabilities::terminal_link::TerminalLinkTarget;
use craic_system::system::materialize::{MaterializedFile, materialize_bytes_for_view};
use craic_system::system::providers::local::LocalProvider;
use craic_system::system::providers::ssh::{SshProvider, SshProviderConfig};
use craic_system::system::transfer::transfer_file_node;
use craic_system::system::{FileNodePath, WorkspacePath};
use craic_system::workspace::WorkspaceEntry;
use craic_ui_containers::docker::{
    self, ComposeProject, ContainerGroup, ContainerInventory, ContainerSummary,
};
use craic_ui_preview::csv_table::{CsvTable, parse_csv_table};
use craic_ui_preview::safetensors_metadata::{metadata_text_from_bytes, read_metadata_header};
use craic_vcs::git::{
    BackgroundPullSubscription, BytesComparison, ChangeListener, ChangeListenerSubscription,
    ChangedFile, Commit, CommitPage, FileComparison, GitCommandEvent, GitCommandGenerator,
    GitRepoHandle, GitSettings, MergeResult, RepoMetadata, RepositorySnapshot, ResetMode,
    WorkspaceRepositoryMetadata, WorkspaceSnapshot, default_commit_summary,
};
use craic_vcs::github::{self, CommitEmailOption, GitHubAuthAccount};
use craic_vcs::gitignore::{self, IgnoreTargetKind};
use craic_vcs::{GitHubAccess, github_access_for_provider};
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject, Sel};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSApplicationTerminateReply, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSButtonType,
    NSCellImagePosition, NSColor, NSControlSize, NSControlStateValueMixed, NSControlStateValueOff,
    NSControlStateValueOn, NSControlTextEditingDelegate, NSDragOperation, NSDraggingInfo, NSEvent,
    NSEventModifierFlags, NSEventType, NSFindPanelAction, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGlassEffectView, NSGlassEffectViewStyle, NSImage,
    NSImageScaling, NSImageView, NSItemBadge, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow, NSLineBreakMode, NSLinkAttributeName,
    NSMenu, NSMenuDelegate, NSMenuItem, NSMenuItemValidation, NSMenuToolbarItem, NSModalResponseOK,
    NSOpenPanel, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPopUpButton,
    NSPopover, NSPopoverBehavior, NSProgressIndicator, NSProgressIndicatorStyle,
    NSRunningApplication, NSSavePanel, NSScrollView, NSSearchField, NSSecureTextField, NSSplitView,
    NSSplitViewController, NSSplitViewDelegate, NSSplitViewDividerStyle, NSSplitViewItem,
    NSTabViewController, NSTabViewControllerTabStyle, NSTabViewItem, NSTableColumn,
    NSTableHeaderView, NSTableRowView, NSTableView, NSTableViewColumnAutoresizingStyle,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewDropOperation, NSTableViewStyle,
    NSTextAlignment, NSTextDelegate, NSTextField, NSTextFieldDelegate, NSTextView,
    NSTextViewDelegate, NSTitlePosition, NSTokenField, NSTokenStyle, NSToolbar, NSToolbarDelegate,
    NSToolbarDisplayMode, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemGroup,
    NSToolbarItemGroupControlRepresentation, NSToolbarItemGroupSelectionMode,
    NSToolbarItemIdentifier, NSToolbarItemVisibilityPriorityHigh,
    NSToolbarItemVisibilityPriorityLow, NSToolbarItemVisibilityPriorityUser,
    NSToolbarSpaceItemIdentifier, NSUserInterfaceItemIdentifier, NSView,
    NSViewBoundsDidChangeNotification, NSViewController, NSWindow, NSWindowDelegate,
    NSWindowOcclusionState, NSWindowStyleMask, NSWindowTitleVisibility, NSWindowToolbarStyle,
    NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_core_foundation::{CFArray, CFData, CFRetained};
use objc2_core_text::{
    CTFont, CTFontManagerCreateFontDescriptorFromData, CTFontManagerCreateFontDescriptorsFromData,
    CTFontManagerRegisterFontDescriptors, CTFontManagerScope,
    CTFontManagerUnregisterFontDescriptors,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSData, NSDate, NSDateFormatter, NSDateFormatterStyle,
    NSEdgeInsets, NSError, NSIndexSet, NSMutableAttributedString, NSNotification,
    NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect, NSRectEdge, NSSize,
    NSString, NSTimer, NSURL, NSUserDefaults,
};
use objc2_pdf_kit::{PDFDisplayDirection, PDFDisplayMode, PDFDocument, PDFView};
use objc2_uniform_type_identifiers::{UTType, UTTypeAudio, UTTypeImage};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationType, WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
    WKWebViewConfiguration,
};
use regex::Regex;
use tokio_util::sync::CancellationToken as WorkspaceCancellationToken;

const WINDOW_WIDTH: f64 = 1440.0;
const WINDOW_HEIGHT: f64 = 920.0;
const SIDEBAR_WIDTH: f64 = 400.0;
const SIDEBAR_MAX_WIDTH: f64 = 480.0;
const WORKSPACE_PICKER_WIDTH: f64 = 360.0;
const WORKSPACE_PICKER_HEIGHT: f64 = 310.0;
const WORKSPACE_ROW_HEIGHT: f64 = 46.0;
const BRANCH_PICKER_WIDTH: f64 = 360.0;
const BRANCH_PICKER_HEIGHT: f64 = 390.0;
const BRANCH_ROW_HEIGHT: f64 = 34.0;
const AUTHOR_PICKER_WIDTH: f64 = 360.0;
const AUTHOR_PICKER_HEIGHT: f64 = 220.0;
const AUTHOR_ROW_HEIGHT: f64 = 52.0;
const FILE_ROW_HEIGHT: f64 = 30.0;
const CONTAINER_ROW_HEIGHT: f64 = 44.0;
const CONTAINER_SOURCE_LIST_HORIZONTAL_INSET: f64 = 16.0;
const CONTAINER_ROW_TRAILING_INSET: f64 = 16.0;
const CONTAINER_STATE_MAX_WIDTH: f64 = 96.0;
const FILE_TREE_ROW_LIMIT: usize = 4_000;
const FILE_CONTENT_PREVIEW_LIMIT: u64 = 8 * 1024 * 1024;
const FONT_CONTENT_PREVIEW_LIMIT: u64 = 32 * 1024 * 1024;
const SQLITE_MATERIALIZE_LIMIT: u64 = 512 * 1024 * 1024;
const SQLITE_CONTROLS_HEIGHT: f64 = 34.0;
const HISTORY_LOADING_ROW_HEIGHT: f64 = 64.0;
const CHANGED_FILE_ROW_HEIGHT: f64 = 36.0;
const CHANGED_FILE_ROW_INSET: f64 = 4.0;
const SELECTION_HEADER_HEIGHT: f64 = 42.0;
const REPOSITORY_CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);
const FILE_PREVIEW_CACHE_CAPACITY: usize = 6;
const HISTORY_PREVIEW_CACHE_CAPACITY: usize = 48;
const AGENT_IMAGE_PREVIEW_LIMIT: u64 = 16 * 1024 * 1024;
const AGENT_IMAGE_CACHE_CAPACITY: usize = 16;
const AGENT_TERMINAL_ROW_HEIGHT: f64 = 74.0;
const AGENT_THREAD_COMPACT_ROW_HEIGHT: f64 = 62.0;
const AGENT_THREAD_PREVIEW_ROW_HEIGHT: f64 = 96.0;
const AGENT_SIDEBAR_ROW_GAP: f64 = 6.0;
const TOOLBAR_WORKSPACE_MIN_WIDTH: f64 = 112.0;
const TOOLBAR_WORKSPACE_MAX_WIDTH: f64 = 208.0;
const TOOLBAR_WORKSPACE: &str = "dev.craic.toolbar.workspace";
const TOOLBAR_PAGES: &str = "dev.craic.toolbar.pages";
const TOOLBAR_BRANCH: &str = "dev.craic.toolbar.branch";
const TOOLBAR_FETCH: &str = "dev.craic.toolbar.fetch";
const TOOLBAR_TERMINAL: &str = "dev.craic.toolbar.terminal";
const TOOLBAR_ADD_ACTION: &str = "dev.craic.toolbar.add-action";
const WINDOW_FRAME_AUTOSAVE: &str = "dev.craic.window.main.frame";
const MAIN_SPLIT_AUTOSAVE: &str = "dev.craic.split.main";
const CHANGES_SPLIT_AUTOSAVE: &str = "dev.craic.split.changes";
const TERMINAL_SPLIT_AUTOSAVE: &str = "dev.craic.split.terminal";
const TERMINAL_AUTO_CLOSE_IDLE_SECONDS: f64 = 60.0;

const NATIVE_LOCAL_SHELL_ACTIVITY_WRAPPER: &str = r#"
monitor_script=$1
shift
sh -c "$monitor_script" craic-terminal-monitor "$$" &
exec "$@"
"#;
const NATIVE_LOCAL_SHELL_ACTIVITY_MONITOR: &str = r#"
root_pid=$1
monitor_pid=$$
last_state=

sleep 0.1
while kill -0 "$root_pid" 2>/dev/null; do
    child_pid="$(pgrep -P "$root_pid" 2>/dev/null | awk -v monitor="$monitor_pid" '$1 != monitor { print $1; exit }')"
    if [ -n "$child_pid" ]; then
        state=active
    else
        state=idle
    fi
    if [ "$state" != "$last_state" ]; then
        printf '\033]0;craic-terminal-activity:%s\007' "$state" > /dev/tty
        last_state=$state
    fi
    sleep 0.5
done
"#;
const ACTIVE_PAGE_DEFAULT: &str = "dev.craic.page.active-index";
const AGENT_MODEL_DEFAULT: &str = "dev.craic.agent.model";
const AGENT_REASONING_DEFAULT: &str = "dev.craic.agent.reasoning";
const AGENT_PERSONALITY_DEFAULT: &str = "dev.craic.agent.personality";
const AGENT_SERVICE_TIER_DEFAULT: &str = "dev.craic.agent.service-tier";
const AGENT_PERMISSIONS_DEFAULT: &str = "dev.craic.agent.permissions";

enum NativePageBadge {
    None,
    Count(usize),
    Indicator,
}

#[derive(Clone, Copy)]
enum NativeRemoteAction {
    Contextual,
    Pull,
    Push,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NativeWebPreviewMode {
    #[default]
    Hidden,
    BesideEditor,
    FullPane,
}

async fn until_workspace_change<T>(
    cancellation: &WorkspaceCancellationToken,
    retired_jobs: &RetiredJobSender,
    label: &'static str,
    mut task: tokio::task::JoinHandle<T>,
) -> Option<Result<T, tokio::task::JoinError>>
where
    T: Send + 'static,
{
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            retired_jobs.retire(label, task).await;
            None
        },
        result = &mut task => Some(result),
    }
}

enum NativeJobWait<T> {
    Completed(Result<T, tokio::task::JoinError>),
    WorkspaceChanged,
    TimedOut,
}

async fn wait_workspace_job<T>(
    cancellation: &WorkspaceCancellationToken,
    retired_jobs: &RetiredJobSender,
    label: &'static str,
    mut task: tokio::task::JoinHandle<T>,
    cancel_requested: Option<&Arc<AtomicBool>>,
) -> NativeJobWait<T>
where
    T: Send + 'static,
{
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            if let Some(cancel_requested) = cancel_requested {
                cancel_requested.store(true, Ordering::SeqCst);
            }
            retired_jobs.retire(label, task).await;
            NativeJobWait::WorkspaceChanged
        },
        _ = tokio::time::sleep(REPOSITORY_CALLBACK_TIMEOUT) => {
            if let Some(cancel_requested) = cancel_requested {
                cancel_requested.store(true, Ordering::SeqCst);
            }
            retired_jobs.retire(label, task).await;
            NativeJobWait::TimedOut
        },
        result = &mut task => NativeJobWait::Completed(result),
    }
}

async fn wait_workspace_future<T, F>(
    cancellation: &WorkspaceCancellationToken,
    retired_jobs: &RetiredJobSender,
    label: &'static str,
    future: F,
) -> NativeJobWait<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    wait_workspace_job(
        cancellation,
        retired_jobs,
        label,
        tokio::spawn(future),
        None,
    )
    .await
}

async fn wait_native_result<T, F>(
    cancellation: &WorkspaceCancellationToken,
    retired_jobs: &RetiredJobSender,
    label: &'static str,
    timeout_message: &'static str,
    future: F,
) -> Option<Result<T, String>>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    match wait_workspace_future(cancellation, retired_jobs, label, future).await {
        NativeJobWait::Completed(Ok(result)) => Some(result),
        NativeJobWait::Completed(Err(error)) => Some(Err(format!("{label} task failed: {error}"))),
        NativeJobWait::WorkspaceChanged => None,
        NativeJobWait::TimedOut => Some(Err(timeout_message.to_string())),
    }
}

async fn wait_file_operation<T>(
    mut events: FileOperationReceiver<T>,
    cancellation: &WorkspaceCancellationToken,
    cancel_requested: Arc<AtomicBool>,
) -> Option<Result<T, String>> {
    let receive = async move {
        while let Some(event) = events.recv().await {
            if let FileOperationEvent::Finished(result) = event {
                return result.map_err(|error| error.to_string());
            }
        }
        Err("File operation ended without a result.".to_string())
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            cancel_requested.store(true, Ordering::SeqCst);
            None
        }
        result = tokio::time::timeout(REPOSITORY_CALLBACK_TIMEOUT, receive) => Some(match result {
            Ok(result) => result,
            Err(_) => {
                cancel_requested.store(true, Ordering::SeqCst);
                Err("File operation timed out.".to_string())
            }
        }),
    }
}
