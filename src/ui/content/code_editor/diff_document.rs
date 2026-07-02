#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum EditorDiffKind {
    Context,
    Added,
    Deleted,
    Missing,
    Fold,
}

#[derive(Clone)]
pub(in crate::ui) struct DiffEditorDocument {
    pub(in crate::ui) rows: Vec<DiffEditorRow>,
    pub(in crate::ui) language: String,
    pub(in crate::ui) source: String,
}

#[derive(Clone)]
pub(in crate::ui) struct DiffEditorRow {
    pub(in crate::ui) number: Option<usize>,
    pub(in crate::ui) text: String,
    pub(in crate::ui) paired_text: String,
    pub(in crate::ui) source_start: Option<usize>,
    pub(in crate::ui) source_end: Option<usize>,
    pub(in crate::ui) kind: EditorDiffKind,
    pub(in crate::ui) fold_index: Option<usize>,
    pub(in crate::ui) fold_expanded: bool,
    pub(in crate::ui) show_fold_control: bool,
}

#[derive(Clone)]
pub(in crate::ui) struct ScrollbarMarker {
    pub(in crate::ui) row: usize,
    pub(in crate::ui) kind: ScrollbarMarkerKind,
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum ScrollbarMarkerKind {
    Added,
    Deleted,
    Mixed,
}
