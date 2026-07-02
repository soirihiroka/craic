use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileNodeCapabilities, FileNodeInfo, FileNodeKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowIgnoreDisplay {
    None,
    Inherited,
    GitIgnored,
}

impl RowIgnoreDisplay {
    pub(super) fn is_ignored(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct RowCapabilities {
    pub(super) readable: bool,
    pub(super) listable: bool,
    pub(super) writable: bool,
    pub(super) creatable: bool,
    pub(super) movable: bool,
    pub(super) deletable: bool,
    pub(super) watchable: bool,
    pub(super) searchable: bool,
    pub(super) open_external: bool,
    pub(super) reveal: bool,
    pub(super) native: bool,
}

impl From<&FileNodeCapabilities> for RowCapabilities {
    fn from(capabilities: &FileNodeCapabilities) -> Self {
        Self {
            readable: capabilities.readable,
            listable: capabilities.listable,
            writable: capabilities.writable,
            creatable: capabilities.creatable,
            movable: capabilities.movable,
            deletable: capabilities.deletable,
            watchable: capabilities.watchable,
            searchable: capabilities.searchable,
            open_external: capabilities.open_external,
            reveal: capabilities.reveal,
            native: capabilities.native,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct BrowserRow {
    pub(super) node_path: FileNodePath,
    pub(super) path: String,
    pub(super) name: String,
    pub(super) depth: usize,
    pub(super) is_dir: bool,
    pub(super) kind: FileNodeKind,
    pub(super) executable: bool,
    pub(super) tree_role: TreeRowRole,
    pub(super) capabilities: RowCapabilities,
    pub(super) ignore: RowIgnoreDisplay,
    pub(super) ignore_known: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TreeRowRole {
    Branch,
    Leaf,
}

impl BrowserRow {
    pub(super) fn from_info(info: FileNodeInfo, depth: usize) -> Self {
        let mut kind = info.kind;
        if matches!(kind, FileNodeKind::File)
            && let Some(format) = crate::system::ArchiveFormat::from_name(&info.display_name)
        {
            kind = FileNodeKind::Archive { format };
        }
        let capabilities = RowCapabilities::from(&info.capabilities);
        let is_dir = kind == FileNodeKind::Directory;
        let tree_role = if is_dir || capabilities.listable {
            TreeRowRole::Branch
        } else {
            TreeRowRole::Leaf
        };
        let executable = info.mode.is_some_and(|mode| mode & 0o111 != 0);
        let mut row = Self {
            path: info.path.display(),
            name: info.display_name,
            depth,
            is_dir,
            kind,
            executable,
            tree_role,
            capabilities,
            ignore: RowIgnoreDisplay::None,
            ignore_known: info.git_ignored.is_some(),
            node_path: info.path,
        };
        if info.git_ignored == Some(true) {
            row.ignore = RowIgnoreDisplay::GitIgnored;
        }
        row
    }
}
