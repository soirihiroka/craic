use crate::system::FileNodePath;
use crate::system::WorkspaceRef;
use crate::system::capabilities::files::{
    FileAccess, FileKind, FileOperationEvent, FileRead, FileReadRequest, FileWriteMode,
    FileWritePayload, FileWriteRequest,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoreTargetKind {
    File,
    Folder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreOption {
    pub label: String,
    pub pattern: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreCheck {
    pub path: String,
    pub is_dir: bool,
}

pub fn options_for_path(path: &str, kind: IgnoreTargetKind) -> Vec<IgnoreOption> {
    let path = normalize_repo_path(path);
    if path.is_empty() {
        return Vec::new();
    }

    let mut options = Vec::new();

    if kind == IgnoreTargetKind::File {
        if let Some(pattern) = extension_pattern(&path) {
            options.push(IgnoreOption {
                label: format!("Ignore All {pattern} Files"),
                pattern,
            });
        }
    }

    if let Some(pattern) = parent_folder_pattern(&path) {
        options.push(IgnoreOption {
            label: if kind == IgnoreTargetKind::Folder {
                "Ignore Parent Folder".to_string()
            } else {
                "Ignore Folder".to_string()
            },
            pattern,
        });
    }

    let pattern = match kind {
        IgnoreTargetKind::File => escape_pattern(&path),
        IgnoreTargetKind::Folder => escape_pattern(&format!("{path}/")),
    };
    options.push(IgnoreOption {
        label: match kind {
            IgnoreTargetKind::File => "Ignore File".to_string(),
            IgnoreTargetKind::Folder => "Ignore Folder".to_string(),
        },
        pattern,
    });

    options
}

pub fn add_pattern_to_workspace(
    files: &dyn FileAccess,
    _workspace: &WorkspaceRef,
    pattern: &str,
) -> Result<String, String> {
    if pattern.is_empty() {
        return Err("Ignore pattern cannot be empty.".to_string());
    }

    let root = files.root();
    let gitignore_path = root.join_child(".gitignore");
    let existing = match read_with_info(files, &gitignore_path, None) {
        Ok(read) if read.info.kind == FileKind::File => read.into_bytes()?,
        Ok(_) => return Err(".gitignore is not a file.".to_string()),
        Err(_) => Vec::new(),
    };

    if contains_pattern(&existing, pattern) {
        return Ok(format!("{pattern} is already in .gitignore."));
    }

    let mut next = existing;
    if next.last().is_some_and(|byte| *byte != b'\n') {
        next.push(b'\n');
    }
    next.extend_from_slice(pattern.as_bytes());
    next.push(b'\n');

    write_file_contents(files, &gitignore_path, next)?;
    Ok(format!("Added {pattern} to .gitignore."))
}

fn read_with_info(
    files: &dyn FileAccess,
    path: &FileNodePath,
    max_bytes: Option<u64>,
) -> Result<FileRead, String> {
    let (sender, receiver) = mpsc::channel();
    files.read_with_info(
        FileReadRequest {
            path: path.clone(),
            max_bytes,
            cancel_requested: None,
        },
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let _ = sender.send(result);
            }
        }),
    );
    receiver
        .recv()
        .map_err(|_| "Read operation did not return a result.".to_string())?
        .map_err(|err| err.to_string())
}

fn write_file_contents(
    files: &dyn FileAccess,
    path: &FileNodePath,
    contents: Vec<u8>,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    files.write_node(
        FileWriteRequest {
            path: path.clone(),
            mode: FileWriteMode::Replace,
            payload: FileWritePayload::File(contents),
            cancel_requested: None,
        },
        Box::new(move |event| {
            if let FileOperationEvent::Finished(result) = event {
                let _ = sender.send(result);
            }
        }),
    );
    receiver
        .recv()
        .map_err(|_| "Write operation did not return a result.".to_string())?
        .map_err(|err| err.to_string())
}

pub fn check_ignored_paths(
    repo_path: &Path,
    checks: &[IgnoreCheck],
) -> Result<HashSet<String>, String> {
    if checks.is_empty() {
        return Ok(HashSet::new());
    }

    let mut child = Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("Failed to start git check-ignore: {err}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "git check-ignore did not expose stdin.".to_string())?;
        stdin
            .write_all(&check_ignore_stdin(checks))
            .map_err(|err| format!("Failed to write git check-ignore input: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to read git check-ignore output: {err}"))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err("git check-ignore failed.".to_string());
    }

    Ok(parse_check_ignore_output(checks, &output.stdout))
}

pub fn check_ignore_stdin(checks: &[IgnoreCheck]) -> Vec<u8> {
    let mut input = Vec::new();
    for check in checks {
        input.extend_from_slice(ignore_check_input(check).as_bytes());
        input.push(0);
    }
    input
}

pub fn parse_check_ignore_output(checks: &[IgnoreCheck], stdout: &[u8]) -> HashSet<String> {
    let check_paths = checks
        .iter()
        .flat_map(|check| {
            [
                (ignore_check_input(check), check.path.clone()),
                (check.path.clone(), check.path.clone()),
            ]
        })
        .collect::<HashMap<_, _>>();
    let mut ignored_paths = HashSet::new();
    for path in stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path);
        let normalized = path.trim_end_matches('/');
        if let Some(check_path) = check_paths
            .get(path.as_ref())
            .or_else(|| check_paths.get(normalized))
        {
            ignored_paths.insert(check_path.clone());
        } else {
            ignored_paths.insert(normalized.to_string());
        }
    }

    ignored_paths
}

fn normalize_repo_path(path: &str) -> String {
    path.trim_matches('/').to_string()
}

fn parent_folder_pattern(path: &str) -> Option<String> {
    let (folder, _) = path.rsplit_once('/')?;
    (!folder.is_empty()).then(|| escape_pattern(&format!("{folder}/")))
}

fn extension_pattern(path: &str) -> Option<String> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| format!("*.{extension}"))
}

fn escape_pattern(pattern: &str) -> String {
    if pattern.starts_with('#') || pattern.starts_with('!') {
        format!("\\{pattern}")
    } else {
        pattern.to_string()
    }
}

fn contains_pattern(contents: &[u8], pattern: &str) -> bool {
    let pattern = pattern.as_bytes();
    contents.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line == pattern
    })
}

fn ignore_check_input(check: &IgnoreCheck) -> String {
    if check.is_dir {
        format!("{}/", check.path)
    } else {
        check.path.clone()
    }
}
