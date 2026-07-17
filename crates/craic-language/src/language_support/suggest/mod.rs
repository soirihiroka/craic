mod rust;

use tree_sitter::Tree;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
}

#[derive(Clone, Debug)]
pub struct CompletionSet {
    pub items: Vec<CompletionItem>,
    pub replacement_start: usize,
    pub replacement_end: usize,
}

pub fn completions(
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
