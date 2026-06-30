use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SystemId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WorkspaceId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HostRef {
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ProviderKind {
    Local,
    Ssh,
    Container,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SystemRef {
    pub(crate) id: SystemId,
    pub(crate) provider_kind: ProviderKind,
    pub(crate) host: Option<HostRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WorkspaceRef {
    pub(crate) id: WorkspaceId,
    pub(crate) root: WorkspacePath,
    pub(crate) display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WorkspacePath {
    pub(crate) absolute: String,
    pub(crate) relative: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SystemPath {
    pub(crate) system: SystemRef,
    pub(crate) workspace: WorkspaceRef,
    pub(crate) path: WorkspacePath,
}

impl SystemId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl WorkspaceId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn for_target(system_id: &SystemId, absolute: &str) -> Self {
        let normalized = normalize_absolute(absolute.to_string());
        Self(format!(
            "ws-{:016x}",
            stable_hash(&format!("{}\0{}", system_id.as_str(), normalized))
        ))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl HostRef {
    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
            Self::Container => "container",
        })
    }
}

impl SystemRef {
    pub(crate) fn new(id: SystemId, provider_kind: ProviderKind, host: Option<HostRef>) -> Self {
        Self {
            id,
            provider_kind,
            host,
        }
    }
}

impl WorkspaceRef {
    pub(crate) fn new(
        id: WorkspaceId,
        root: WorkspacePath,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            id,
            root,
            display_name: display_name.into(),
        }
    }

    pub(crate) fn path(&self, relative: impl AsRef<str>) -> WorkspacePath {
        WorkspacePath::from_workspace_relative(&self.root, relative.as_ref())
    }

    pub(crate) fn root_system_path(&self, system: &SystemRef) -> SystemPath {
        SystemPath::new(system.clone(), self.clone(), self.root.clone())
    }
}

impl WorkspacePath {
    /// Absolute paths are absolute on the target system, not necessarily on this machine.
    pub(crate) fn from_absolute(absolute: impl Into<String>) -> Self {
        Self {
            absolute: normalize_absolute(absolute.into()),
            relative: None,
        }
    }

    pub(crate) fn from_workspace_relative(root: &WorkspacePath, relative: &str) -> Self {
        let relative = normalize_relative(relative);
        let absolute = if relative.is_empty() {
            root.absolute.clone()
        } else {
            join_target_path(&root.absolute, &relative)
        };
        Self {
            absolute,
            relative: (!relative.is_empty()).then_some(relative),
        }
    }

    pub(crate) fn relative_or_empty(&self) -> &str {
        self.relative.as_deref().unwrap_or("")
    }

    pub(crate) fn display(&self) -> &str {
        self.relative.as_deref().unwrap_or(&self.absolute)
    }

    pub(crate) fn file_name(&self) -> Option<&str> {
        self.display()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
    }

    pub(crate) fn parent(&self, root: &WorkspacePath) -> Option<Self> {
        let relative = self.relative.as_deref()?;
        let (parent, _) = relative.rsplit_once('/')?;
        Some(Self::from_workspace_relative(root, parent))
    }

    pub(crate) fn join(&self, child: &str) -> Self {
        let child = normalize_relative(child);
        let absolute = join_target_path(&self.absolute, &child);
        let relative = self
            .relative
            .as_ref()
            .map(|relative| join_target_path(relative, &child));
        Self { absolute, relative }
    }
}

impl SystemPath {
    pub(crate) fn new(system: SystemRef, workspace: WorkspaceRef, path: WorkspacePath) -> Self {
        Self {
            system,
            workspace,
            path,
        }
    }

    pub(crate) fn display(&self) -> String {
        format!(
            "{}:{}",
            self.system.id,
            self.path.relative.as_deref().unwrap_or(&self.path.absolute)
        )
    }
}

pub(crate) fn workspace_id_for_absolute_path(path: &Path) -> WorkspaceId {
    WorkspaceId::for_target(
        &SystemId::new("local"),
        &pathbuf_to_target_absolute(path.to_path_buf()),
    )
}

pub(crate) fn path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn pathbuf_to_target_absolute(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_absolute(value: String) -> String {
    if value.is_empty() {
        "/".to_string()
    } else {
        value.replace('\\', "/")
    }
}

pub(crate) fn normalize_relative(value: &str) -> String {
    let mut parts = Vec::new();
    let value = value.replace('\\', "/");
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn join_target_path(base: &str, child: &str) -> String {
    let child = normalize_relative(child);
    if child.is_empty() {
        return base.trim_end_matches('/').to_string();
    }
    let base = base.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        format!("/{child}")
    } else {
        format!("{base}/{child}")
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
