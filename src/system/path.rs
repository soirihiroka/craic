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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarBz2,
    Iso,
    Img,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FileNodePath {
    pub(crate) nodes: Vec<FileNodeRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FileNodeRef {
    Root {
        root_id: String,
        system_id: SystemId,
    },
    NativeChild {
        name: String,
    },
    ArchiveRoot {
        format: ArchiveFormat,
    },
    ArchiveChild {
        name: String,
    },
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

impl fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::TarBz2 => "tar.bz2",
            Self::Iso => "iso",
            Self::Img => "img",
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

    pub(crate) fn root_node_path(&self, system: &SystemRef) -> FileNodePath {
        FileNodePath::root(system, self)
    }

    pub(crate) fn node_path(&self, system: &SystemRef, relative: impl AsRef<str>) -> FileNodePath {
        FileNodePath::from_workspace_relative(system, self, relative.as_ref())
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

impl ArchiveFormat {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
            Some(Self::TarXz)
        } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
            Some(Self::TarBz2)
        } else if lower.ends_with(".zip") {
            Some(Self::Zip)
        } else if lower.ends_with(".tar") {
            Some(Self::Tar)
        } else if lower.ends_with(".iso") {
            Some(Self::Iso)
        } else if lower.ends_with(".img") {
            Some(Self::Img)
        } else {
            None
        }
    }
}

impl FileNodePath {
    pub(crate) fn root(system: &SystemRef, workspace: &WorkspaceRef) -> Self {
        Self {
            nodes: vec![FileNodeRef::Root {
                root_id: workspace.id.to_string(),
                system_id: system.id.clone(),
            }],
        }
    }

    pub(crate) fn from_workspace_relative(
        system: &SystemRef,
        workspace: &WorkspaceRef,
        relative: &str,
    ) -> Self {
        let mut path = Self::root(system, workspace);
        for name in normalize_relative(relative)
            .split('/')
            .filter(|name| !name.is_empty())
        {
            path.nodes.push(FileNodeRef::NativeChild {
                name: name.to_string(),
            });
        }
        path
    }

    pub(crate) fn root_ref(&self) -> Option<(&str, &SystemId)> {
        match self.nodes.first() {
            Some(FileNodeRef::Root { root_id, system_id }) => Some((root_id, system_id)),
            _ => None,
        }
    }

    pub(crate) fn is_root(&self) -> bool {
        self.nodes.len() == 1 && matches!(self.nodes.first(), Some(FileNodeRef::Root { .. }))
    }

    pub(crate) fn is_native(&self) -> bool {
        self.nodes.iter().all(|node| {
            matches!(
                node,
                FileNodeRef::Root { .. } | FileNodeRef::NativeChild { .. }
            )
        })
    }

    pub(crate) fn contains_archive(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| matches!(node, FileNodeRef::ArchiveRoot { .. }))
    }

    pub(crate) fn open_archive(&self, format: ArchiveFormat) -> Self {
        let mut path = self.clone();
        path.nodes.push(FileNodeRef::ArchiveRoot { format });
        path
    }

    pub(crate) fn join_child(&self, name: impl Into<String>) -> Self {
        let mut path = self.clone();
        for name in normalize_relative(&name.into())
            .split('/')
            .filter(|name| !name.is_empty())
        {
            path.nodes.push(if path.contains_archive() {
                FileNodeRef::ArchiveChild {
                    name: name.to_string(),
                }
            } else {
                FileNodeRef::NativeChild {
                    name: name.to_string(),
                }
            });
        }
        path
    }

    pub(crate) fn native_relative(&self) -> Option<String> {
        let mut parts = Vec::new();
        for node in self.nodes.iter().skip(1) {
            match node {
                FileNodeRef::NativeChild { name } => parts.push(name.as_str()),
                FileNodeRef::Root { .. } => return None,
                FileNodeRef::ArchiveRoot { .. } | FileNodeRef::ArchiveChild { .. } => return None,
            }
        }
        Some(parts.join("/"))
    }

    pub(crate) fn to_workspace_path(&self, workspace: &WorkspaceRef) -> Option<WorkspacePath> {
        let (root_id, _) = self.root_ref()?;
        if root_id != workspace.id.as_str() {
            return None;
        }
        self.native_relative()
            .map(|relative| WorkspacePath::from_workspace_relative(&workspace.root, &relative))
    }

    pub(crate) fn display(&self) -> String {
        let mut output = String::new();
        let mut after_archive_root = false;
        for node in self.nodes.iter().skip(1) {
            match node {
                FileNodeRef::NativeChild { name } => {
                    if !output.is_empty() {
                        output.push('/');
                    }
                    output.push_str(name);
                    after_archive_root = false;
                }
                FileNodeRef::ArchiveRoot { .. } => {
                    output.push_str("!/");
                    after_archive_root = true;
                }
                FileNodeRef::ArchiveChild { name } => {
                    if !output.is_empty() && !output.ends_with('/') && !after_archive_root {
                        output.push('/');
                    }
                    output.push_str(name);
                    after_archive_root = false;
                }
                FileNodeRef::Root { .. } => {}
            }
        }
        output
    }

    pub(crate) fn file_name(&self) -> Option<&str> {
        self.nodes.iter().rev().find_map(|node| match node {
            FileNodeRef::NativeChild { name } | FileNodeRef::ArchiveChild { name } => {
                Some(name.as_str())
            }
            FileNodeRef::Root { .. } | FileNodeRef::ArchiveRoot { .. } => None,
        })
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        if self.nodes.len() <= 1 {
            return None;
        }
        let mut path = self.clone();
        path.nodes.pop();
        Some(path)
    }

    pub(crate) fn is_child_of(&self, parent: &Self) -> bool {
        self.nodes.len() > parent.nodes.len() && self.nodes.starts_with(&parent.nodes)
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
