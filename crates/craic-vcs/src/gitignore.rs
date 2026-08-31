use crate::system::capabilities::files::{
    FileAccess, FileKind, FileOperation, FileReadRequest, FileWriteMode, FileWritePayload,
    FileWriteRequest, wait_file_operation,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};
use std::thread;

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IgnoreOptions {
    pub direct: Option<IgnoreOption>,
    pub folders: Vec<IgnoreOption>,
    pub extension: Option<IgnoreOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreCheck {
    pub path: String,
    pub is_dir: bool,
}

pub fn options_for_path(path: &str, kind: IgnoreTargetKind) -> IgnoreOptions {
    let path = normalize_repo_path(path);
    if path.is_empty()
        || kind == IgnoreTargetKind::File
            && path
                .rsplit('/')
                .next()
                .is_some_and(|name| name == ".gitignore")
    {
        return IgnoreOptions::default();
    }

    let direct = (kind == IgnoreTargetKind::File).then(|| IgnoreOption {
        label: "Ignore File (Add to .gitignore)".to_string(),
        pattern: escape_pattern(&path),
    });
    let folder = match kind {
        IgnoreTargetKind::File => path.rsplit_once('/').map(|(folder, _)| folder),
        IgnoreTargetKind::Folder => Some(path.as_str()),
    };
    let folders = folder
        .into_iter()
        .flat_map(folder_options)
        .collect::<Vec<_>>();
    let extension = (kind == IgnoreTargetKind::File)
        .then(|| extension_option(&path))
        .flatten();

    IgnoreOptions {
        direct,
        folders,
        extension,
    }
}

pub fn add_pattern_to_workspace(
    files: Arc<dyn FileAccess>,
    pattern: String,
) -> mpsc::Receiver<Result<String, String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    if pattern.is_empty() {
        let _ = sender.send(Err("Ignore pattern cannot be empty.".to_string()));
        return receiver;
    }

    let root = files.root();
    let gitignore_path = root.join_child(".gitignore");
    let read_events = files.read_with_info_events(FileReadRequest {
        path: gitignore_path.clone(),
        max_bytes: None,
        cancel_requested: None,
    });
    thread::spawn(move || {
        let existing = match wait_file_operation(read_events, FileOperation::Read) {
            Ok(read) if read.info.kind == FileKind::File => match read.into_bytes() {
                Ok(bytes) => bytes,
                Err(err) => {
                    let _ = sender.send(Err(err));
                    return;
                }
            },
            Ok(_) => {
                let _ = sender.send(Err(".gitignore is not a file.".to_string()));
                return;
            }
            Err(_) => Vec::new(),
        };

        if contains_pattern(&existing, &pattern) {
            let _ = sender.send(Ok(format!("{pattern} is already in .gitignore.")));
            return;
        }

        let mut next = existing;
        if next.last().is_some_and(|byte| *byte != b'\n') {
            next.push(b'\n');
        }
        next.extend_from_slice(pattern.as_bytes());
        next.push(b'\n');
        let result = wait_file_operation(
            files.write_node_events(FileWriteRequest {
                path: gitignore_path,
                mode: FileWriteMode::Replace,
                payload: FileWritePayload::File(next),
                cancel_requested: None,
            }),
            FileOperation::Write,
        )
        .map(|_| format!("Added {pattern} to .gitignore."))
        .map_err(|err| err.to_string());
        let _ = sender.send(result);
    });
    receiver
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

fn folder_options(folder: &str) -> Vec<IgnoreOption> {
    let mut folder = folder;
    let mut options = Vec::new();
    while !folder.is_empty() {
        options.push(IgnoreOption {
            label: format!("/{folder}"),
            pattern: format!("{}/", escape_pattern(folder)),
        });
        folder = folder.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    options
}

fn extension_option(path: &str) -> Option<IgnoreOption> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let index = file_name.rfind('.')?;
    if index == 0 || index + 1 == file_name.len() {
        return None;
    }
    let extension = &file_name[index..];
    Some(IgnoreOption {
        label: format!("Ignore All {extension} Files (Add to .gitignore)"),
        pattern: format!("*{}", escape_pattern(extension)),
    })
}

fn escape_pattern(pattern: &str) -> String {
    let mut escaped = String::with_capacity(pattern.len());
    for character in pattern.chars() {
        if matches!(character, '[' | ']' | '!' | '*' | '#' | '?') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
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
