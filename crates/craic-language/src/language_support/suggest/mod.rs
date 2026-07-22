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

pub trait CompletionService: Sync {
    fn completions(&self, tree: &Tree, source: &str, cursor: usize) -> Option<CompletionSet>;
}

pub(super) struct RustCompletion;
pub(super) static RUST_COMPLETION: RustCompletion = RustCompletion;

impl CompletionService for RustCompletion {
    fn completions(&self, tree: &Tree, source: &str, cursor: usize) -> Option<CompletionSet> {
        rust::completions(tree, source, cursor)
    }
}

pub fn completions(
    service: &dyn CompletionService,
    tree: &Tree,
    source: &str,
    cursor: usize,
) -> Option<CompletionSet> {
    service.completions(tree, source, cursor)
}
