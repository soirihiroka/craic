use crate::system::path::WorkspacePath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalLinkTarget {
    Workspace(WorkspacePath),
    External(WorkspacePath),
}

pub trait TerminalLinkAccess: Send + Sync {
    fn resolve_file(&self, launch_dir: &str, target: &str) -> Result<TerminalLinkTarget, String>;
}

pub fn resolve_terminal_file<F>(
    workspace_root: &WorkspacePath,
    launch_dir: &str,
    target: &str,
    mut exists: F,
) -> Result<TerminalLinkTarget, String>
where
    F: FnMut(&WorkspacePath) -> bool,
{
    let target = normalize_path_text(target);
    if target.is_empty() {
        return Err("No file path was found under the cursor.".to_string());
    }

    let workspace_root_absolute = normalize_absolute_path(&workspace_root.absolute);
    if is_absolute_path(&target) {
        let absolute = normalize_absolute_path(&target);
        return if let Some(relative) =
            workspace_relative_for_absolute(&workspace_root_absolute, &absolute)
        {
            let path = WorkspacePath::from_workspace_relative(workspace_root, &relative);
            if exists(&path) {
                Ok(TerminalLinkTarget::Workspace(path))
            } else {
                Err(format!("{target} was not found in the current workspace."))
            }
        } else {
            let path = WorkspacePath::from_absolute(absolute);
            if exists(&path) {
                Ok(TerminalLinkTarget::External(path))
            } else {
                Err(format!("{target} was not found."))
            }
        };
    }

    let launch_candidate = join_target_path(launch_dir, &target);
    if let Some(relative) =
        workspace_relative_for_absolute(&workspace_root_absolute, &launch_candidate)
    {
        let path = WorkspacePath::from_workspace_relative(workspace_root, &relative);
        if exists(&path) {
            return Ok(TerminalLinkTarget::Workspace(path));
        }
    } else {
        let path = WorkspacePath::from_absolute(launch_candidate);
        if exists(&path) {
            return Ok(TerminalLinkTarget::External(path));
        }
    }

    if let Some(relative) = normalize_relative_path(&target) {
        let path = WorkspacePath::from_workspace_relative(workspace_root, &relative);
        if exists(&path) {
            return Ok(TerminalLinkTarget::Workspace(path));
        }
    }

    Err(format!("{target} was not found."))
}

fn normalize_path_text(value: &str) -> String {
    value.trim().replace('\\', "/")
}

fn is_absolute_path(value: &str) -> bool {
    value.starts_with('/')
}

fn normalize_absolute_path(value: &str) -> String {
    let value = normalize_path_text(value);
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn normalize_relative_path(value: &str) -> Option<String> {
    let value = normalize_path_text(value);
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn join_target_path(base: &str, relative: &str) -> String {
    if is_absolute_path(relative) {
        return normalize_absolute_path(relative);
    }

    let base = normalize_absolute_path(base);
    normalize_absolute_path(&format!("{}/{}", base.trim_end_matches('/'), relative))
}

fn workspace_relative_for_absolute(
    workspace_root_absolute: &str,
    absolute: &str,
) -> Option<String> {
    let workspace_root = workspace_root_absolute.trim_end_matches('/');
    let absolute = normalize_absolute_path(absolute);
    if absolute == workspace_root {
        return Some(String::new());
    }

    let prefix = if workspace_root == "/" {
        "/".to_string()
    } else {
        format!("{workspace_root}/")
    };
    absolute
        .strip_prefix(&prefix)
        .map(|relative| relative.to_string())
}
