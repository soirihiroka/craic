use crate::gitignore;
use crate::system::capabilities::files::{
    DirectoryEntry, DirectoryListing, FileAccess, FileKind, FileMetadata, FileRead,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileWatchCallback,
    FileWatchChanges, FileWatchRequest, FileWatchSubscription,
};
use crate::system::path::{SystemPath, SystemRef, WorkspacePath, WorkspaceRef};
use gtk::prelude::*;
use gtk::{gio, glib};
use regex::{Regex, RegexBuilder};
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use walkdir::WalkDir;

const LOCAL_FILE_MONITOR_RATE_LIMIT_MS: i32 = 250;
const LOCAL_FILE_MONITOR_STOP_POLL_MS: u64 = 250;
const LOCAL_FILE_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(750);

#[derive(Clone, Debug)]
pub(crate) struct LocalFileAccess {
    system: SystemRef,
    workspace: WorkspaceRef,
    root_path: PathBuf,
}

impl LocalFileAccess {
    pub(crate) fn new(system: SystemRef, workspace: WorkspaceRef) -> Self {
        let root_path = PathBuf::from(&workspace.root.absolute);
        Self {
            system,
            workspace,
            root_path,
        }
    }

    fn local_path(&self, path: &WorkspacePath) -> Result<PathBuf, String> {
        let local_path = match path.relative.as_deref() {
            Some(relative) if !relative.is_empty() => self.root_path.join(relative),
            _ => PathBuf::from(&path.absolute),
        };

        if local_path.starts_with(&self.root_path) {
            Ok(local_path)
        } else {
            Err("Path is outside the workspace.".to_string())
        }
    }

    fn system_path(&self, path: WorkspacePath) -> SystemPath {
        SystemPath::new(self.system.clone(), self.workspace.clone(), path)
    }

    fn workspace_path_for_local(&self, path: &Path) -> WorkspacePath {
        let relative = path
            .strip_prefix(&self.root_path)
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        WorkspacePath::from_workspace_relative(&self.workspace.root, &relative)
    }
}

impl FileAccess for LocalFileAccess {
    fn workspace(&self) -> WorkspaceRef {
        self.workspace.clone()
    }

    fn watch(
        &self,
        request: FileWatchRequest,
        callback: FileWatchCallback,
    ) -> Result<FileWatchSubscription, String> {
        let requested_paths = if request.paths.is_empty() {
            vec![self.workspace.root.clone()]
        } else {
            request.paths.clone()
        };
        let local_paths = requested_paths
            .iter()
            .map(|path| {
                self.local_path(path)
                    .map(|local_path| (path.clone(), local_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let recursive = request.recursive;
        let label = if requested_paths.len() == 1 {
            format!(
                "local-file:{}:{}",
                self.workspace.display_name,
                requested_paths[0].display()
            )
        } else {
            format!(
                "local-file:{}:{}paths",
                self.workspace.display_name,
                requested_paths.len()
            )
        };
        log::info!(
            "local file watch registered workspace={} paths={} recursive={} mode=gio",
            self.workspace.display_name,
            local_paths.len(),
            request.recursive
        );
        let root_path = self.root_path.clone();
        let workspace_root = self.workspace.root.clone();
        Ok(FileWatchSubscription::spawn_thread(
            label,
            move |stop_receiver| {
                run_local_gio_file_monitor(
                    local_paths,
                    root_path,
                    workspace_root,
                    recursive,
                    stop_receiver,
                    callback,
                );
            },
        ))
    }

    fn metadata(&self, path: &WorkspacePath) -> Result<FileMetadata, String> {
        let local_path = self.local_path(path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to inspect {}: {err}", path.display()))?;
        Ok(FileMetadata {
            path: self.system_path(path.clone()),
            kind: file_kind(&metadata),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
            executable: is_executable(&metadata),
        })
    }

    fn list_dirs(&self, paths: &[WorkspacePath]) -> Result<Vec<DirectoryListing>, String> {
        let mut listings = Vec::new();
        for path in paths {
            let local_path = self.local_path(path)?;
            let mut entries = Vec::new();
            for entry in fs::read_dir(&local_path)
                .map_err(|err| format!("Unable to list {}: {err}", path.display()))?
            {
                let entry =
                    entry.map_err(|err| format!("Unable to read directory entry: {err}"))?;
                let metadata = entry
                    .metadata()
                    .map_err(|err| format!("Unable to inspect directory entry: {err}"))?;
                let path = self.workspace_path_for_local(&entry.path());
                entries.push(DirectoryEntry {
                    path: self.system_path(path),
                    name: entry.file_name().to_string_lossy().to_string(),
                    kind: file_kind(&metadata),
                    len: metadata.len(),
                    modified: metadata.modified().ok(),
                    executable: is_executable(&metadata),
                    git_ignored: None,
                });
            }
            listings.push(DirectoryListing {
                path: path.clone(),
                entries,
            });
        }
        apply_local_git_ignore(&self.root_path, &mut listings);
        Ok(listings)
    }

    fn read_with_metadata(
        &self,
        path: &WorkspacePath,
        max_bytes: Option<u64>,
    ) -> Result<FileRead, String> {
        let local_path = self.local_path(path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to read {}: {err}", path.display()))?;
        let kind = file_kind(&metadata);
        let bytes = if kind == FileKind::File
            && max_bytes.is_none_or(|max_bytes| metadata.len() <= max_bytes)
        {
            Some(
                fs::read(&local_path)
                    .map_err(|err| format!("Unable to read {}: {err}", path.display()))?,
            )
        } else {
            None
        };
        Ok(FileRead {
            metadata: FileMetadata {
                path: self.system_path(path.clone()),
                kind,
                len: metadata.len(),
                modified: metadata.modified().ok(),
                readonly: metadata.permissions().readonly(),
                executable: is_executable(&metadata),
            },
            bytes,
        })
    }

    fn write_bytes(&self, path: &WorkspacePath, contents: &[u8]) -> Result<(), String> {
        let local_path = self.local_path(path)?;
        fs::write(&local_path, contents)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))
    }

    fn write_text(&self, path: &WorkspacePath, contents: &str) -> Result<(), String> {
        let local_path = self.local_path(path)?;
        let metadata = fs::metadata(&local_path)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))?;
        if !metadata.is_file() {
            return Err("Select a file to edit.".to_string());
        }
        fs::write(&local_path, contents)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))
    }

    fn create_file(&self, path: &WorkspacePath) -> Result<(), String> {
        let local_path = self.local_path(path)?;
        if let Some(parent) = local_path.parent()
            && !parent.is_dir()
        {
            return Err("Parent folder does not exist.".to_string());
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&local_path)
            .map(|_| ())
            .map_err(|err| format!("Unable to create file: {err}"))
    }

    fn create_dir(&self, path: &WorkspacePath) -> Result<(), String> {
        let local_path = self.local_path(path)?;
        fs::create_dir(&local_path).map_err(|err| format!("Unable to create folder: {err}"))
    }

    fn rename(&self, source: &WorkspacePath, destination: &WorkspacePath) -> Result<(), String> {
        let source = self.local_path(source)?;
        let destination = self.local_path(destination)?;
        if destination.exists() {
            return Err("Destination already exists.".to_string());
        }
        fs::rename(&source, &destination).map_err(|err| format!("Unable to rename: {err}"))
    }

    fn delete(&self, path: &WorkspacePath) -> Result<(), String> {
        let local_path = self.local_path(path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to inspect path: {err}"))?;
        if metadata.is_dir() {
            fs::remove_dir_all(&local_path).map_err(|err| format!("Unable to delete folder: {err}"))
        } else {
            fs::remove_file(&local_path).map_err(|err| format!("Unable to delete file: {err}"))
        }
    }

    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String> {
        let root = self.local_path(&query.root)?;
        let matcher = build_search_regex(&query)?;
        let excluded_names = query.excluded_names.clone();
        let mut matches = Vec::new();
        let mut limited = false;

        log::info!(
            "file search start provider=local workspace={} query_len={} root={}",
            self.workspace.display_name,
            query.query.len(),
            query.root.display()
        );

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || entry
                        .file_name()
                        .to_str()
                        .is_none_or(|name| !excluded_names.iter().any(|excluded| excluded == name))
            })
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > query.max_file_bytes {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let path = self.workspace_path_for_local(entry.path());
            collect_file_matches(&path, &text, &matcher, &query, &mut matches, &mut limited);
            if limited {
                break;
            }
        }

        matches.sort_by(|left, right| {
            left.path
                .relative_or_empty()
                .cmp(right.path.relative_or_empty())
                .then_with(|| left.start.cmp(&right.start))
        });
        log::info!(
            "file search complete provider=local workspace={} matches={} limited={}",
            self.workspace.display_name,
            matches.len(),
            limited
        );
        Ok(FileSearchOutput { matches, limited })
    }
}

fn local_file_signature(path: &Path) -> Result<Option<FileSignature>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(FileSignature {
            kind: file_kind(&metadata),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "Unable to inspect watched path {}: {err}",
            path.display()
        )),
    }
}

fn run_local_gio_file_monitor(
    local_paths: Vec<(WorkspacePath, PathBuf)>,
    root_path: PathBuf,
    workspace_root: WorkspacePath,
    recursive: bool,
    stop_receiver: mpsc::Receiver<()>,
    callback: FileWatchCallback,
) {
    let display_paths = local_paths
        .iter()
        .map(|(_, path)| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let context = glib::MainContext::new();
    let context_result = context.with_thread_default(|| {
        let main_loop = glib::MainLoop::new(Some(&context), false);
        let flags = gio::FileMonitorFlags::WATCH_MOVES | gio::FileMonitorFlags::SEND_MOVED;
        let mut monitors = Vec::new();

        for (_, local_path) in &local_paths {
            let file = gio::File::for_path(local_path);
            let monitor_result = if recursive || local_path.is_dir() {
                file.monitor_directory(flags, None::<&gio::Cancellable>)
            } else {
                file.monitor_file(flags, None::<&gio::Cancellable>)
            };
            let monitor = match monitor_result {
                Ok(monitor) => monitor,
                Err(err) => {
                    log::warn!(
                        "local gio file watch unavailable path={} watched_paths={} err={err}; falling back to polling",
                        local_path.display(),
                        local_paths.len()
                    );
                    run_local_signature_file_monitor(
                        local_paths.clone(),
                        stop_receiver,
                        callback,
                    );
                    return;
                }
            };

            monitor.set_rate_limit(LOCAL_FILE_MONITOR_RATE_LIMIT_MS);
            monitor.connect_changed({
                let root_path = root_path.clone();
                let workspace_root = workspace_root.clone();
                let callback = Arc::clone(&callback);

                move |_, file, other_file, event_type| {
                    if !local_file_monitor_event_should_notify(event_type) {
                        return;
                    }

                    let changes =
                        local_file_monitor_changes(&root_path, &workspace_root, file, other_file);
                    if changes.is_empty() {
                        return;
                    }

                    log::debug!(
                        "local gio file watch event event_type={event_type:?} changed_paths={}",
                        changes.len()
                    );
                    callback(changes);
                }
            });
            monitors.push(monitor);
        }

        if monitors.is_empty() {
            log::warn!("local gio file watch has no paths; falling back to polling");
            run_local_signature_file_monitor(local_paths, stop_receiver, callback);
            return;
        }

        log::info!(
            "local gio file watch started watched_paths={}",
            monitors.len()
        );

        let stop_loop = main_loop.clone();
        let stop_source = glib::timeout_source_new(
            Duration::from_millis(LOCAL_FILE_MONITOR_STOP_POLL_MS),
            Some("craic-local-file-monitor-stop"),
            glib::Priority::DEFAULT,
            move || {
                if stop_receiver.try_recv().is_ok() {
                    stop_loop.quit();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
        stop_source.attach(Some(&context));

        main_loop.run();
        for monitor in monitors {
            monitor.cancel();
        }
    });

    if let Err(err) = context_result {
        log::warn!(
            "local gio file watch failed to attach thread context paths={} err={err}",
            display_paths
        );
    }
}

fn run_local_signature_file_monitor(
    local_paths: Vec<(WorkspacePath, PathBuf)>,
    stop_receiver: mpsc::Receiver<()>,
    callback: FileWatchCallback,
) {
    let mut previous_signatures: Option<HashMap<WorkspacePath, Option<FileSignature>>> = None;

    loop {
        let mut next_signatures = HashMap::new();
        for (workspace_path, local_path) in &local_paths {
            match local_file_signature(local_path) {
                Ok(signature) => {
                    next_signatures.insert(workspace_path.clone(), signature);
                }
                Err(err) => {
                    log::warn!(
                        "local file watch fallback metadata failed path={} err={err}",
                        local_path.display()
                    );
                    next_signatures.insert(workspace_path.clone(), None);
                }
            }
        }

        if let Some(previous) = &previous_signatures {
            let changes = changed_signature_paths(previous, &next_signatures);
            if !changes.is_empty() {
                log::debug!(
                    "local file watch fallback change detected watched_paths={} changed_paths={}",
                    next_signatures.len(),
                    changes.len()
                );
                callback(changes);
            }
        } else {
            log::debug!(
                "local file watch fallback initial snapshot watched_paths={}",
                next_signatures.len()
            );
        }
        previous_signatures = Some(next_signatures);

        if stop_receiver
            .recv_timeout(LOCAL_FILE_FALLBACK_POLL_INTERVAL)
            .is_ok()
        {
            break;
        }
    }
}

fn changed_signature_paths(
    previous: &HashMap<WorkspacePath, Option<FileSignature>>,
    next: &HashMap<WorkspacePath, Option<FileSignature>>,
) -> FileWatchChanges {
    let mut changes = FileWatchChanges::new();
    for (path, next_signature) in next {
        if previous.get(path) != Some(next_signature) {
            changes.insert(path.clone());
        }
    }
    for path in previous.keys() {
        if !next.contains_key(path) {
            changes.insert(path.clone());
        }
    }
    changes
}

fn local_file_monitor_event_should_notify(event_type: gio::FileMonitorEvent) -> bool {
    !matches!(
        event_type,
        gio::FileMonitorEvent::AttributeChanged
            | gio::FileMonitorEvent::PreUnmount
            | gio::FileMonitorEvent::Unmounted
    )
}

fn local_file_monitor_changes(
    root_path: &Path,
    workspace_root: &WorkspacePath,
    file: &gio::File,
    other_file: Option<&gio::File>,
) -> FileWatchChanges {
    let mut changes = FileWatchChanges::new();
    collect_local_file_monitor_path(&mut changes, root_path, workspace_root, file);
    if let Some(other_file) = other_file {
        collect_local_file_monitor_path(&mut changes, root_path, workspace_root, other_file);
    }
    changes
}

fn collect_local_file_monitor_path(
    changes: &mut FileWatchChanges,
    root_path: &Path,
    workspace_root: &WorkspacePath,
    file: &gio::File,
) {
    let Some(path) = file.path() else {
        return;
    };
    if let Some(workspace_path) = workspace_path_for_local_root(root_path, workspace_root, &path) {
        changes.insert(workspace_path);
    }
}

fn workspace_path_for_local_root(
    root_path: &Path,
    workspace_root: &WorkspacePath,
    path: &Path,
) -> Option<WorkspacePath> {
    let relative = path
        .strip_prefix(root_path)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some(WorkspacePath::from_workspace_relative(
        workspace_root,
        &relative,
    ))
}

fn apply_local_git_ignore(root_path: &Path, listings: &mut [DirectoryListing]) {
    let checks = listings
        .iter()
        .flat_map(|listing| listing.entries.iter())
        .filter_map(ignore_check_for_entry)
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return;
    }

    let ignored_paths = match gitignore::check_ignored_paths(root_path, &checks) {
        Ok(ignored_paths) => ignored_paths,
        Err(err) => {
            log::debug!(
                "local list dir git ignore unavailable root={} err={err}",
                root_path.display()
            );
            return;
        }
    };
    for listing in listings {
        apply_git_ignore_flags(&mut listing.entries, &ignored_paths);
    }
}

fn ignore_check_for_entry(entry: &DirectoryEntry) -> Option<gitignore::IgnoreCheck> {
    let path = entry.path.path.relative_or_empty();
    (!path.is_empty()).then(|| gitignore::IgnoreCheck {
        path: path.to_string(),
        is_dir: entry.kind == FileKind::Directory,
    })
}

fn apply_git_ignore_flags(entries: &mut [DirectoryEntry], ignored_paths: &HashSet<String>) {
    for entry in entries {
        let path = entry.path.path.relative_or_empty();
        if !path.is_empty() {
            entry.git_ignored = Some(ignored_paths.contains(path));
        }
    }
}

fn file_kind(metadata: &fs::Metadata) -> FileKind {
    if metadata.is_file() {
        FileKind::File
    } else if metadata.is_dir() {
        FileKind::Directory
    } else if metadata.file_type().is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    }
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn build_search_regex(query: &FileSearchQuery) -> Result<Regex, String> {
    let mut pattern = if query.regex {
        query.query.clone()
    } else {
        regex::escape(&query.query)
    };
    if query.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .dot_matches_new_line(true)
        .build()
        .map_err(|err| format!("Invalid search pattern: {err}"))
}

fn collect_file_matches(
    path: &WorkspacePath,
    text: &str,
    matcher: &Regex,
    query: &FileSearchQuery,
    matches: &mut Vec<FileSearchMatch>,
    limited: &mut bool,
) {
    for found in matcher.find_iter(text) {
        if found.is_empty() {
            continue;
        }
        matches.push(FileSearchMatch {
            path: path.clone(),
            line_number: line_number_for_offset(text, found.start()),
            start: found.start(),
            end: found.end(),
            line_text: search_match_preview(text, found.start(), found.end()),
        });
        if matches.len() >= query.max_results {
            *limited = true;
            return;
        }
    }
}

fn line_number_for_offset(text: &str, offset: usize) -> u64 {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u64
        + 1
}

fn search_match_preview(text: &str, start: usize, end: usize) -> String {
    let line_start = text[..start.min(text.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = text[end.min(text.len())..]
        .find('\n')
        .map(|index| end.min(text.len()) + index)
        .unwrap_or(text.len());
    let preview = text[line_start..line_end].trim().replace(['\r', '\n'], " ");
    truncate_search_text(&preview)
}

fn truncate_search_text(text: &str) -> String {
    const MAX_CHARS: usize = 180;

    let mut output = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == MAX_CHARS {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}
