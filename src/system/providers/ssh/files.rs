use super::{SshCommandRunner, remote_workspace_path, shell_quote, workspace_path_for_remote};
use crate::gitignore;
use crate::system::capabilities::files::{
    DirectoryEntry, DirectoryListing, FileAccess, FileKind, FileMetadata, FileRead,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileWatchCallback,
    FileWatchRequest, FileWatchSubscription,
};
use crate::system::path::{SystemPath, SystemRef, WorkspacePath, WorkspaceRef};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

const SSH_FILE_WATCH_POLL_INTERVAL: Duration = Duration::from_secs(60);
const SSH_LIST_DIR_CACHE_TTL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub(crate) struct SshFileAccess {
    system: SystemRef,
    workspace: WorkspaceRef,
    runner: SshCommandRunner,
    list_dir_cache: Arc<Mutex<SshListDirCache>>,
}

#[derive(Debug, Default)]
struct SshListDirCache {
    entries: HashMap<String, CachedDirectoryListing>,
}

#[derive(Clone, Debug)]
struct CachedDirectoryListing {
    listing: DirectoryListing,
    cached_at: Instant,
}

#[derive(Deserialize)]
struct RemoteSearchOutput {
    matches: Vec<RemoteSearchMatch>,
    limited: bool,
}

#[derive(Deserialize)]
struct RemoteSearchMatch {
    path: String,
    line_number: u64,
    start: usize,
    end: usize,
    line_text: String,
}

impl SshFileAccess {
    pub(crate) fn new(
        system: SystemRef,
        workspace: WorkspaceRef,
        runner: SshCommandRunner,
    ) -> Self {
        Self {
            system,
            workspace,
            runner,
            list_dir_cache: Arc::new(Mutex::new(SshListDirCache::default())),
        }
    }
}

impl SshListDirCache {
    fn fresh_listing(&self, path: &WorkspacePath, now: Instant) -> Option<DirectoryListing> {
        self.entries
            .get(&ssh_list_dir_cache_key(path))
            .filter(|entry| now.duration_since(entry.cached_at) <= SSH_LIST_DIR_CACHE_TTL)
            .map(|entry| entry.listing.clone())
    }

    fn listing(&self, path: &WorkspacePath) -> Option<DirectoryListing> {
        self.entries
            .get(&ssh_list_dir_cache_key(path))
            .map(|entry| entry.listing.clone())
    }

    fn insert_listings(&mut self, listings: Vec<DirectoryListing>) {
        let cached_at = Instant::now();
        for listing in listings {
            self.entries.insert(
                ssh_list_dir_cache_key(&listing.path),
                CachedDirectoryListing { listing, cached_at },
            );
        }
    }
}

fn ssh_list_dir_cache_key(path: &WorkspacePath) -> String {
    path.relative_or_empty().to_string()
}

impl FileAccess for SshFileAccess {
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
        let requested = requested_paths
            .iter()
            .map(|path| (path.clone(), remote_workspace_path(&self.workspace, path)))
            .collect::<Vec<_>>();
        let runner = self.runner.clone();
        let label = if requested_paths.len() == 1 {
            format!(
                "ssh-file:{}:{}",
                self.workspace.display_name,
                requested_paths[0].display()
            )
        } else {
            format!(
                "ssh-file:{}:{}paths",
                self.workspace.display_name,
                requested_paths.len()
            )
        };
        log::info!(
            "ssh file watch registered workspace={} paths={} recursive={} interval_secs={}",
            self.workspace.display_name,
            requested.len(),
            request.recursive,
            SSH_FILE_WATCH_POLL_INTERVAL.as_secs()
        );
        Ok(FileWatchSubscription::spawn_signature_map_loop(
            label,
            SSH_FILE_WATCH_POLL_INTERVAL,
            move || remote_file_signatures(&runner, &requested),
            callback,
        ))
    }

    fn metadata(&self, path: &WorkspacePath) -> Result<FileMetadata, String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = shell_metadata_script(&remote_path);
        let raw = self.runner.run_text("file metadata", &script)?;
        parse_metadata_line(&self.system, &self.workspace, &remote_path, raw.trim_end())
    }

    fn list_dirs(&self, paths: &[WorkspacePath]) -> Result<Vec<DirectoryListing>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let mut cache = self
            .list_dir_cache
            .lock()
            .map_err(|_| "SSH directory list cache is unavailable.".to_string())?;
        let missing_paths = paths
            .iter()
            .filter(|path| cache.fresh_listing(path, Instant::now()).is_none())
            .cloned()
            .collect::<Vec<_>>();

        if !missing_paths.is_empty() {
            log::debug!(
                "ssh list directories cache miss workspace={} requested={} missing={}",
                self.workspace.display_name,
                paths.len(),
                missing_paths.len()
            );
            let requested = missing_paths
                .iter()
                .map(|path| (path.clone(), remote_workspace_path(&self.workspace, path)))
                .collect::<Vec<_>>();
            let remote_paths = requested
                .iter()
                .map(|(_, remote_path)| remote_path.clone())
                .collect::<Vec<_>>();
            let script = shell_list_dirs_script(&remote_paths);
            let raw = self.runner.run_text("list directories", &script)?;
            let mut listings =
                parse_directory_output(&self.system, &self.workspace, &requested, &raw)?;
            apply_remote_git_ignore(&self.runner, &self.workspace, &mut listings);
            for listing in &mut listings {
                sort_directory_entries(&mut listing.entries);
            }
            cache.insert_listings(listings);
        } else {
            log::debug!(
                "ssh list directories cache hit workspace={} requested={}",
                self.workspace.display_name,
                paths.len()
            );
        }

        let listings = paths
            .iter()
            .filter_map(|path| cache.listing(path))
            .collect::<Vec<_>>();
        Ok(listings)
    }

    fn read_with_metadata(
        &self,
        path: &WorkspacePath,
        max_bytes: Option<u64>,
    ) -> Result<FileRead, String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = shell_read_with_metadata_script(&remote_path, max_bytes);
        let output = self.runner.run_script("read file with metadata", &script)?;
        parse_read_output(&self.system, &self.workspace, &remote_path, output.stdout)
    }

    fn write_bytes(&self, path: &WorkspacePath, contents: &[u8]) -> Result<(), String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = format!("cat > {}", shell_quote(&remote_path));
        self.runner
            .run_script_with_stdin("write file", &script, Some(contents))
            .map(|_| ())
    }

    fn write_text(&self, path: &WorkspacePath, contents: &str) -> Result<(), String> {
        self.write_bytes(path, contents.as_bytes())
    }

    fn create_file(&self, path: &WorkspacePath) -> Result<(), String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = format!("p={}; (set -C; : > \"$p\")", shell_quote(&remote_path));
        self.runner.run_script("create file", &script).map(|_| ())
    }

    fn create_dir(&self, path: &WorkspacePath) -> Result<(), String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = format!("mkdir -- {}", shell_quote(&remote_path));
        self.runner
            .run_script("create directory", &script)
            .map(|_| ())
    }

    fn rename(&self, source: &WorkspacePath, destination: &WorkspacePath) -> Result<(), String> {
        let source = remote_workspace_path(&self.workspace, source);
        let destination = remote_workspace_path(&self.workspace, destination);
        let script = format!(
            "[ ! -e {dst} ] && mv -- {src} {dst}",
            src = shell_quote(&source),
            dst = shell_quote(&destination)
        );
        self.runner.run_script("rename path", &script).map(|_| ())
    }

    fn delete(&self, path: &WorkspacePath) -> Result<(), String> {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = format!("rm -rf -- {}", shell_quote(&remote_path));
        self.runner.run_script("delete path", &script).map(|_| ())
    }

    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String> {
        let root = remote_workspace_path(&self.workspace, &query.root);
        let excluded_names =
            serde_json::to_string(&query.excluded_names).unwrap_or_else(|_| json!([]).to_string());
        let script = format!(
            "python3 -c {} {} {} {} {} {} {} {} {}",
            shell_quote(SEARCH_SCRIPT),
            shell_quote(&root),
            shell_quote(&query.query),
            shell_quote(&excluded_names),
            if query.case_sensitive { "1" } else { "0" },
            if query.whole_word { "1" } else { "0" },
            if query.regex { "1" } else { "0" },
            query.max_results,
            query.max_file_bytes,
        );
        let raw = self.runner.run_text("search files", &script)?;
        let output: RemoteSearchOutput = serde_json::from_str(&raw)
            .map_err(|err| format!("Invalid remote search response: {err}"))?;
        Ok(FileSearchOutput {
            matches: output
                .matches
                .into_iter()
                .map(|found| FileSearchMatch {
                    path: workspace_path_for_remote(&self.workspace, &found.path),
                    line_number: found.line_number,
                    start: found.start,
                    end: found.end,
                    line_text: found.line_text,
                })
                .collect(),
            limited: output.limited,
        })
    }
}

fn parse_kind(kind: &str) -> FileKind {
    match kind {
        "f" | "regular file" | "file" => FileKind::File,
        "d" | "directory" | "dir" => FileKind::Directory,
        "l" | "symbolic link" | "symlink" => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

fn remote_time(value: Option<f64>) -> Option<std::time::SystemTime> {
    value.map(|secs| UNIX_EPOCH + Duration::from_secs_f64(secs.max(0.0)))
}

fn shell_metadata_script(remote_path: &str) -> String {
    format!(
        r#"p={}
if [ -d "$p" ]; then kind=dir; elif [ -f "$p" ]; then kind=file; elif [ -L "$p" ]; then kind=symlink; else kind=other; fi
metadata=$(stat -Lc '%s %Y' -- "$p") || exit 1
set -- $metadata
if [ -w "$p" ]; then readonly=0; else readonly=1; fi
if [ -x "$p" ]; then executable=1; else executable=0; fi
printf '%s\t%s\t%s\t%s\t%s\n' "$kind" "$1" "$2" "$readonly" "$executable""#,
        shell_quote(remote_path)
    )
}

fn remote_file_signatures(
    runner: &SshCommandRunner,
    requested: &[(WorkspacePath, String)],
) -> Result<HashMap<WorkspacePath, Option<FileSignature>>, String> {
    let remote_paths = requested
        .iter()
        .map(|(_, remote_path)| remote_path.clone())
        .collect::<Vec<_>>();
    let script = shell_file_signatures_script(&remote_paths);
    let raw = runner.run_text("file watch metadata", &script)?;
    parse_file_signature_lines(requested, &raw)
}

fn shell_file_signatures_script(remote_paths: &[String]) -> String {
    let mut script = String::from("set --");
    for remote_path in remote_paths {
        script.push(' ');
        script.push_str(&shell_quote(remote_path));
    }
    script.push_str(
        r#"
for p do
  if [ ! -e "$p" ] && [ ! -L "$p" ]; then
    printf 'CRAIC-WATCH\037%s\037missing\n' "$p"
    continue
  fi
  if [ -d "$p" ]; then kind=dir; elif [ -f "$p" ]; then kind=file; elif [ -L "$p" ]; then kind=symlink; else kind=other; fi
  metadata=$(stat -Lc '%s %Y' -- "$p") || { printf 'CRAIC-WATCH\037%s\037missing\n' "$p"; continue; }
  len=${metadata%% *}
  modified=${metadata#* }
  printf 'CRAIC-WATCH\037%s\037present\037%s\037%s\037%s\n' "$p" "$kind" "$len" "$modified"
done"#,
    );
    script
}

fn parse_file_signature_lines(
    requested: &[(WorkspacePath, String)],
    raw: &str,
) -> Result<HashMap<WorkspacePath, Option<FileSignature>>, String> {
    let mut signatures = requested
        .iter()
        .map(|(path, _)| (path.clone(), None))
        .collect::<HashMap<_, _>>();

    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\x1f');
        if fields.next() != Some("CRAIC-WATCH") {
            continue;
        }
        let remote_path = fields.next().unwrap_or_default();
        let state = fields.next().unwrap_or_default();
        let Some((workspace_path, _)) = requested
            .iter()
            .find(|(_, requested_remote_path)| requested_remote_path == remote_path)
        else {
            continue;
        };

        match state {
            "missing" => {
                signatures.insert(workspace_path.clone(), None);
            }
            "present" => {
                let kind = parse_kind(fields.next().unwrap_or_default());
                let len = fields
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or_else(|| "Invalid remote file watch metadata length.".to_string())?;
                let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
                signatures.insert(
                    workspace_path.clone(),
                    Some(FileSignature {
                        kind,
                        len,
                        modified: remote_time(modified),
                    }),
                );
            }
            _ => return Err("Invalid remote file watch metadata state.".to_string()),
        }
    }

    Ok(signatures)
}

fn shell_list_dirs_script(remote_paths: &[String]) -> String {
    let mut script = String::from("set --");
    for remote_path in remote_paths {
        script.push(' ');
        script.push_str(&shell_quote(remote_path));
    }
    script.push_str(
        r#"
for p do
  printf 'CRAIC-DIR\037%s\n' "$p"
  find "$p" -mindepth 1 -maxdepth 1 -printf 'CRAIC-ENTRY\037%f\037%p\037%y\037%s\037%T@\037%m\n' 2>/dev/null || true
done"#,
    );
    script
}

fn shell_read_with_metadata_script(remote_path: &str, max_bytes: Option<u64>) -> String {
    let max_bytes = max_bytes.map(|value| value.to_string()).unwrap_or_default();
    format!(
        r#"p={}
max={}
if [ -d "$p" ]; then kind=dir; elif [ -f "$p" ]; then kind=file; elif [ -L "$p" ]; then kind=symlink; else kind=other; fi
metadata=$(stat -Lc '%s %Y' -- "$p") || exit 1
set -- $metadata
len=$1
mtime=$2
if [ -w "$p" ]; then readonly=0; else readonly=1; fi
if [ -x "$p" ]; then executable=1; else executable=0; fi
readable=0
if [ "$kind" = file ]; then
    if [ -z "$max" ] || [ "$len" -le "$max" ]; then readable=1; fi
fi
printf 'CRAIC-FILE-READ\t%s\t%s\t%s\t%s\t%s\t%s\n' "$kind" "$len" "$mtime" "$readonly" "$executable" "$readable"
if [ "$readable" = 1 ]; then cat -- "$p"; fi"#,
        shell_quote(remote_path),
        shell_quote(&max_bytes)
    )
}

fn parse_metadata_line(
    system: &SystemRef,
    workspace: &WorkspaceRef,
    remote_path: &str,
    line: &str,
) -> Result<FileMetadata, String> {
    let mut fields = line.split('\t');
    let kind = fields.next().unwrap_or_default();
    let len = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "Invalid remote metadata length.".to_string())?;
    let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
    let readonly = fields.next().unwrap_or_default() == "1";
    let executable = fields.next().unwrap_or_default() == "1";
    let path = workspace_path_for_remote(workspace, remote_path);
    Ok(FileMetadata {
        path: SystemPath::new(system.clone(), workspace.clone(), path),
        kind: parse_kind(kind),
        len,
        modified: remote_time(modified),
        readonly,
        executable,
    })
}

fn parse_directory_output(
    system: &SystemRef,
    workspace: &WorkspaceRef,
    requested: &[(WorkspacePath, String)],
    raw: &str,
) -> Result<Vec<DirectoryListing>, String> {
    let mut listings = requested
        .iter()
        .map(|(path, _)| DirectoryListing {
            path: path.clone(),
            entries: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut current_index = None;

    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\x1f');
        let marker = fields.next().unwrap_or_default();
        if marker == "CRAIC-DIR" {
            let remote_path = fields.next().unwrap_or_default();
            current_index = requested
                .iter()
                .position(|(_, requested_remote_path)| requested_remote_path == remote_path);
            continue;
        }
        if marker != "CRAIC-ENTRY" {
            continue;
        }
        let Some(index) = current_index else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(path) = fields.next() else {
            continue;
        };
        let kind = parse_kind(fields.next().unwrap_or_default());
        let len = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
        let executable = fields
            .next()
            .and_then(|value| u32::from_str_radix(value, 8).ok())
            .is_some_and(|mode| mode & 0o111 != 0);
        let workspace_path = workspace_path_for_remote(workspace, path);
        listings[index].entries.push(DirectoryEntry {
            path: SystemPath::new(system.clone(), workspace.clone(), workspace_path),
            name: name.to_string(),
            kind,
            len,
            modified: remote_time(modified),
            executable,
            git_ignored: None,
        });
    }
    Ok(listings)
}

fn apply_remote_git_ignore(
    runner: &SshCommandRunner,
    workspace: &WorkspaceRef,
    listings: &mut [DirectoryListing],
) {
    let checks = listings
        .iter()
        .flat_map(|listing| listing.entries.iter())
        .filter_map(ignore_check_for_entry)
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return;
    }

    let script = shell_check_ignore_script(&workspace.root.absolute);
    let output = match runner.run_script_with_stdin(
        "list directory git ignore",
        &script,
        Some(&gitignore::check_ignore_stdin(&checks)),
    ) {
        Ok(output) => output,
        Err(err) => {
            log::debug!(
                "ssh list dir git ignore unavailable workspace={} err={err}",
                workspace.display_name
            );
            return;
        }
    };
    let ignored_paths = gitignore::parse_check_ignore_output(&checks, &output.stdout);
    for listing in listings {
        apply_git_ignore_flags(&mut listing.entries, &ignored_paths);
    }
}

fn sort_directory_entries(entries: &mut [DirectoryEntry]) {
    entries.sort_by(|left, right| {
        right
            .kind
            .eq(&FileKind::Directory)
            .cmp(&left.kind.eq(&FileKind::Directory))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

fn shell_check_ignore_script(root: &str) -> String {
    format!(
        "cd {} && git check-ignore --stdin -z; status=$?; if [ \"$status\" -eq 1 ]; then exit 0; fi; exit \"$status\"",
        shell_quote(root)
    )
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

fn parse_read_output(
    system: &SystemRef,
    workspace: &WorkspaceRef,
    remote_path: &str,
    stdout: Vec<u8>,
) -> Result<FileRead, String> {
    let Some(header_end) = stdout.iter().position(|byte| *byte == b'\n') else {
        return Err("Invalid remote file read response.".to_string());
    };
    let header = std::str::from_utf8(&stdout[..header_end])
        .map_err(|_| "Invalid remote file read header.".to_string())?;
    let mut fields = header.split('\t');
    if fields.next() != Some("CRAIC-FILE-READ") {
        return Err("Invalid remote file read marker.".to_string());
    }
    let kind = fields.next().unwrap_or_default();
    let len = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "Invalid remote file read length.".to_string())?;
    let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
    let readonly = fields.next().unwrap_or_default() == "1";
    let executable = fields.next().unwrap_or_default() == "1";
    let readable = fields.next().unwrap_or_default() == "1";
    let path = workspace_path_for_remote(workspace, remote_path);
    let metadata = FileMetadata {
        path: SystemPath::new(system.clone(), workspace.clone(), path),
        kind: parse_kind(kind),
        len,
        modified: remote_time(modified),
        readonly,
        executable,
    };
    let bytes = readable.then(|| stdout[header_end + 1..].to_vec());
    Ok(FileRead { metadata, bytes })
}

const SEARCH_SCRIPT: &str = r#"
import json, os, re, sys
root, query = sys.argv[1], sys.argv[2]
skip = set(json.loads(sys.argv[3]))
case_sensitive = sys.argv[4] == '1'
whole_word = sys.argv[5] == '1'
is_regex = sys.argv[6] == '1'
max_results = int(sys.argv[7])
max_file_bytes = int(sys.argv[8])
pattern = query if is_regex else re.escape(query)
if whole_word:
    pattern = r'\b(?:' + pattern + r')\b'
flags = re.MULTILINE | re.DOTALL
if not case_sensitive:
    flags |= re.IGNORECASE
rx = re.compile(pattern, flags)
matches = []
limited = False
def preview_for_match(text, start, end):
    line_start = text.rfind('\n', 0, start) + 1
    line_end = text.find('\n', end)
    if line_end == -1:
        line_end = len(text)
    return text[line_start:line_end].strip().replace('\r', ' ').replace('\n', ' ')[:180]
for base, dirs, files in os.walk(root):
    dirs[:] = [d for d in dirs if d not in skip]
    for name in files:
        if name in skip:
            continue
        path = os.path.join(base, name)
        try:
            if os.path.getsize(path) > max_file_bytes:
                continue
            data = open(path, 'rb').read()
        except OSError:
            continue
        if b'\0' in data:
            continue
        try:
            text = data.decode('utf-8')
        except UnicodeDecodeError:
            continue
        for found in rx.finditer(text):
            if found.start() == found.end():
                continue
            matches.append({
                'path': path,
                'line_number': text.count('\n', 0, found.start()) + 1,
                'start': found.start(),
                'end': found.end(),
                'line_text': preview_for_match(text, found.start(), found.end()),
            })
            if len(matches) >= max_results:
                limited = True
                print(json.dumps({'matches': matches, 'limited': limited}))
                raise SystemExit(0)
print(json.dumps({'matches': matches, 'limited': limited}))
"#;
