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

#[derive(Clone, PartialEq, Eq)]
pub(super) struct BrowserRow {
    pub(super) path: String,
    pub(super) name: String,
    pub(super) depth: usize,
    pub(super) is_dir: bool,
    pub(super) executable: bool,
    pub(super) tree_role: TreeRowRole,
    pub(super) ignore: RowIgnoreDisplay,
    pub(super) ignore_known: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TreeRowRole {
    Branch,
    Leaf,
}

impl BrowserRow {
    pub(super) fn folder(path: String, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            depth,
            is_dir: true,
            executable: false,
            tree_role: TreeRowRole::Branch,
            ignore: RowIgnoreDisplay::None,
            ignore_known: false,
        }
    }

    pub(super) fn file(path: String, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            depth,
            is_dir: false,
            executable: false,
            tree_role: TreeRowRole::Leaf,
            ignore: RowIgnoreDisplay::None,
            ignore_known: false,
        }
    }

    pub(super) fn search_file_group(path: String, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            depth,
            is_dir: false,
            executable: false,
            tree_role: TreeRowRole::Branch,
            ignore: RowIgnoreDisplay::None,
            ignore_known: false,
        }
    }
}
