mod highlight;
mod newline;
mod suggest;

pub(crate) use highlight::{
    HighlightRange, SyntaxHighlighter, SyntaxIssue, apply_edit_to_ranges, language_hint_from_path,
};
pub(crate) use newline::{NewlineContext, enter_newline};
pub(crate) use suggest::{CompletionItem, CompletionSet};
