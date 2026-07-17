use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileNodeCapabilities, FileNodeInfo, FileNodeKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowIgnoreDisplay {
    None,
    Inherited,
    GitIgnored,
}

impl RowIgnoreDisplay {
    pub fn is_ignored(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowCapabilities {
    pub readable: bool,
    pub listable: bool,
    pub writable: bool,
    pub creatable: bool,
    pub movable: bool,
    pub deletable: bool,
    pub watchable: bool,
    pub searchable: bool,
    pub open_external: bool,
    pub reveal: bool,
    pub native: bool,
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
pub struct BrowserRow {
    pub node_path: FileNodePath,
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub kind: FileNodeKind,
    pub executable: bool,
    pub tree_role: TreeRowRole,
    pub capabilities: RowCapabilities,
    pub ignore: RowIgnoreDisplay,
    pub ignore_known: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeRowRole {
    Branch,
    Leaf,
}

impl BrowserRow {
    pub fn from_info(info: FileNodeInfo, depth: usize) -> Self {
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
