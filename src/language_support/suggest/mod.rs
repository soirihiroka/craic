mod rust;

use tree_sitter::Tree;

#[derive(Clone, Debug)]
pub(crate) struct CompletionItem {
    pub(crate) label: String,
    pub(crate) insert_text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CompletionSet {
    pub(crate) items: Vec<CompletionItem>,
    pub(crate) replacement_start: usize,
    pub(crate) replacement_end: usize,
}

pub(crate) fn completions(
    language_name: &str,
    tree: &Tree,
    source: &str,
    cursor: usize,
) -> Option<CompletionSet> {
    match language_name {
        "rust" | "rs" => rust::completions(tree, source, cursor),
        _ => None,
    }
}
