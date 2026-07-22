mod highlight;
mod newline;
mod registry;
mod suggest;

pub use highlight::{
    CsvSyntax, HighlightRange, PlainSyntax, SyntaxHighlighter, SyntaxIssue, SyntaxSupport,
    TreeSitterSyntax, apply_edit_to_ranges, language_id_from_path,
};
pub use newline::NewlineService;
pub use newline::{NewlineContext, enter_newline};
pub use registry::{
    LANGUAGES, LanguageSupport, LintKind, SpellcheckMode, language_support, language_support_for_id,
};
pub use suggest::{CompletionItem, CompletionService, CompletionSet};
