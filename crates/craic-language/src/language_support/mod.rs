mod highlight;
mod newline;
mod suggest;

pub use highlight::{
    HighlightRange, SyntaxHighlighter, SyntaxIssue, apply_edit_to_ranges, language_hint_from_path,
};
pub use newline::{NewlineContext, enter_newline};
pub use suggest::{CompletionItem, CompletionSet};
