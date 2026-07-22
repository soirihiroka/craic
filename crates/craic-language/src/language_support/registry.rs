use craic_file_support::LanguageId;
use tree_sitter::Language;

use super::highlight::{CsvSyntax, PlainSyntax, SyntaxSupport, TreeSitterSyntax};
use super::newline::{NewlineService, PLAIN_TEXT_NEWLINE, RUST_NEWLINE};
use super::suggest::{CompletionService, RUST_COMPLETION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellcheckMode {
    Disabled,
    Markup,
    QuotedValues,
    FullDocument,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LintKind {
    None,
    Markdown,
}

pub struct LanguageSupport {
    pub id: LanguageId,
    pub aliases: &'static [&'static str],
    pub syntax: &'static dyn SyntaxSupport,
    pub newline: &'static dyn NewlineService,
    pub completion: Option<&'static dyn CompletionService>,
    pub line_comment: Option<&'static str>,
    pub spellcheck: SpellcheckMode,
    pub lint: LintKind,
}

static PLAIN: PlainSyntax = PlainSyntax;
static CSV: CsvSyntax = CsvSyntax;

macro_rules! language_fn {
    ($name:ident, $language:expr) => {
        fn $name() -> Language {
            $language.into()
        }
    };
}
language_fn!(bash_language, tree_sitter_bash::LANGUAGE);
language_fn!(c_language, tree_sitter_c::LANGUAGE);
language_fn!(caddy_language, tree_sitter_caddy::LANGUAGE);
language_fn!(cpp_language, tree_sitter_cpp::LANGUAGE);
language_fn!(css_language, tree_sitter_css::LANGUAGE);
language_fn!(cuda_language, tree_sitter_cuda::LANGUAGE);
language_fn!(go_language, tree_sitter_go::LANGUAGE);
language_fn!(html_language, tree_sitter_html::LANGUAGE);
language_fn!(xml_language, tree_sitter_xml::LANGUAGE_XML);
language_fn!(java_language, tree_sitter_java::LANGUAGE);
language_fn!(javascript_language, tree_sitter_javascript::LANGUAGE);
language_fn!(json_language, tree_sitter_json::LANGUAGE);
language_fn!(make_language, tree_sitter_make::LANGUAGE);
language_fn!(powershell_language, tree_sitter_powershell::LANGUAGE);
language_fn!(hlsl_language, tree_sitter_hlsl::LANGUAGE_HLSL);
language_fn!(slang_language, tree_sitter_slang::LANGUAGE_SLANG);
language_fn!(kotlin_language, tree_sitter_kotlin_ng::LANGUAGE);
language_fn!(markdown_language, tree_sitter_md::LANGUAGE);
language_fn!(markdown_inline_language, tree_sitter_md::INLINE_LANGUAGE);
language_fn!(python_language, tree_sitter_python::LANGUAGE);
language_fn!(ruby_language, tree_sitter_ruby::LANGUAGE);
language_fn!(rst_language, tree_sitter_rst::LANGUAGE);
language_fn!(rust_language, tree_sitter_rust::LANGUAGE);
language_fn!(scheme_language, tree_sitter_scheme::LANGUAGE);
language_fn!(ini_language, tree_sitter_ini::LANGUAGE);
language_fn!(toml_language, tree_sitter_toml_ng::LANGUAGE);
language_fn!(
    typescript_language,
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT
);
language_fn!(tsx_language, tree_sitter_typescript::LANGUAGE_TSX);
language_fn!(yaml_language, tree_sitter_yaml::LANGUAGE);

const RST_QUERY: &str = include_str!("queries/rst.scm");
const RUST_FOLDS: &[&str] = &[
    "function_item",
    "impl_item",
    "trait_item",
    "struct_item",
    "enum_item",
    "mod_item",
    "macro_definition",
    "match_block",
];
const SCRIPT_FOLDS: &[&str] = &[
    "function_declaration",
    "function",
    "arrow_function",
    "method_definition",
    "class_declaration",
    "class",
    "interface_declaration",
    "enum_declaration",
    "type_alias_declaration",
];
const PYTHON_FOLDS: &[&str] = &[
    "function_definition",
    "class_definition",
    "decorated_definition",
    "for_statement",
    "while_statement",
    "if_statement",
    "with_statement",
    "try_statement",
    "match_statement",
];
const C_FOLDS: &[&str] = &[
    "function_definition",
    "struct_specifier",
    "union_specifier",
    "enum_specifier",
];

macro_rules! syntax { ($name:ident, $lang:ident, [$($query:expr),*], $injection:expr, $folds:expr) => { static $name: TreeSitterSyntax = TreeSitterSyntax { language: $lang, highlight_query_parts: &[$($query),*], injection_query: $injection, fold_nodes: $folds }; }; }
syntax!(
    BASH,
    bash_language,
    [tree_sitter_bash::HIGHLIGHT_QUERY],
    None,
    &[
        "function_definition",
        "if_statement",
        "for_statement",
        "while_statement",
        "case_statement",
        "subshell",
        "compound_statement"
    ]
);
syntax!(
    C,
    c_language,
    [tree_sitter_c::HIGHLIGHT_QUERY],
    None,
    C_FOLDS
);
syntax!(CADDY, caddy_language, [], None, &[]);
syntax!(
    CPP,
    cpp_language,
    [
        tree_sitter_c::HIGHLIGHT_QUERY,
        tree_sitter_cpp::HIGHLIGHT_QUERY
    ],
    None,
    &[
        "function_definition",
        "struct_specifier",
        "union_specifier",
        "enum_specifier",
        "namespace_definition",
        "class_specifier",
        "template_declaration"
    ]
);
syntax!(
    CSS,
    css_language,
    [tree_sitter_css::HIGHLIGHTS_QUERY],
    None,
    &["rule_set", "media_statement", "supports_statement"]
);
syntax!(
    CUDA,
    cuda_language,
    [tree_sitter_cuda::HIGHLIGHTS_QUERY],
    None,
    C_FOLDS
);
syntax!(
    GO,
    go_language,
    [tree_sitter_go::HIGHLIGHTS_QUERY],
    None,
    &[
        "function_declaration",
        "method_declaration",
        "type_declaration",
        "struct_type",
        "interface_type",
        "literal_value"
    ]
);
syntax!(
    HTML,
    html_language,
    [tree_sitter_html::HIGHLIGHTS_QUERY],
    Some(tree_sitter_html::INJECTIONS_QUERY),
    &["element", "script_element", "style_element"]
);
syntax!(
    XML,
    xml_language,
    [tree_sitter_xml::XML_HIGHLIGHT_QUERY],
    None,
    &["element"]
);
syntax!(
    JAVA,
    java_language,
    [tree_sitter_java::HIGHLIGHTS_QUERY],
    None,
    &[
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
        "method_declaration",
        "constructor_declaration"
    ]
);
syntax!(
    JAVASCRIPT,
    javascript_language,
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
    ],
    None,
    SCRIPT_FOLDS
);
syntax!(
    JSON,
    json_language,
    [tree_sitter_json::HIGHLIGHTS_QUERY],
    None,
    &["object", "array"]
);
syntax!(
    MAKE,
    make_language,
    [tree_sitter_make::HIGHLIGHTS_QUERY],
    None,
    &[]
);
syntax!(POWERSHELL, powershell_language, [], None, &[]);
syntax!(HLSL, hlsl_language, [], None, &[]);
syntax!(SLANG, slang_language, [], None, &[]);
syntax!(KOTLIN, kotlin_language, [], None, &[]);
syntax!(
    MARKDOWN,
    markdown_language,
    [tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
    Some(tree_sitter_md::INJECTION_QUERY_BLOCK),
    &["section", "fenced_code_block", "list", "block_quote"]
);
syntax!(
    MARKDOWN_INLINE,
    markdown_inline_language,
    [tree_sitter_md::HIGHLIGHT_QUERY_INLINE],
    Some(tree_sitter_md::INJECTION_QUERY_INLINE),
    &[]
);
syntax!(
    PYTHON,
    python_language,
    [tree_sitter_python::HIGHLIGHTS_QUERY],
    None,
    PYTHON_FOLDS
);
syntax!(
    RUBY,
    ruby_language,
    [tree_sitter_ruby::HIGHLIGHTS_QUERY],
    None,
    &[
        "method",
        "singleton_method",
        "class",
        "module",
        "do_block",
        "begin",
        "if",
        "case"
    ]
);
syntax!(RST, rst_language, [RST_QUERY], None, &[]);
syntax!(
    RUST,
    rust_language,
    [tree_sitter_rust::HIGHLIGHTS_QUERY],
    None,
    RUST_FOLDS
);
syntax!(
    SCHEME,
    scheme_language,
    [tree_sitter_scheme::HIGHLIGHTS_QUERY],
    None,
    &[]
);
syntax!(
    INI,
    ini_language,
    [tree_sitter_ini::HIGHLIGHTS_QUERY],
    None,
    &[]
);
syntax!(
    TOML,
    toml_language,
    [tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
    None,
    &[
        "table",
        "array_table",
        "table_array_element",
        "inline_table",
        "array"
    ]
);
syntax!(
    TYPESCRIPT,
    typescript_language,
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    ],
    None,
    SCRIPT_FOLDS
);
syntax!(
    TSX,
    tsx_language,
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    ],
    None,
    SCRIPT_FOLDS
);
syntax!(
    YAML,
    yaml_language,
    [tree_sitter_yaml::HIGHLIGHTS_QUERY],
    None,
    &["block_mapping", "block_sequence"]
);

macro_rules! entry { ($id:ident, [$($alias:expr),*], $syntax:expr, $comment:expr, $spell:ident, $lint:ident) => { LanguageSupport { id: LanguageId::$id, aliases: &[$($alias),*], syntax: $syntax, newline: &PLAIN_TEXT_NEWLINE, completion: None, line_comment: $comment, spellcheck: SpellcheckMode::$spell, lint: LintKind::$lint } }; }
macro_rules! plain { ($id:ident, [$($alias:expr),*]) => { entry!($id, [$($alias),*], &PLAIN, None, FullDocument, None) }; }

pub static LANGUAGES: &[LanguageSupport] = &[
    entry!(
        PlainText,
        ["text", "plain", "plaintext", "txt"],
        &PLAIN,
        None,
        FullDocument,
        None
    ),
    plain!(Astro, ["astro"]),
    entry!(
        Bash,
        ["bash", "sh", "shell", "zsh", "ignore"],
        &BASH,
        Some("#"),
        Disabled,
        None
    ),
    plain!(Bazel, ["bazel", "bzl"]),
    entry!(C, ["c", "h"], &C, Some("//"), Disabled, None),
    entry!(
        Caddy,
        ["caddy", "caddyfile"],
        &CADDY,
        Some("#"),
        Disabled,
        None
    ),
    plain!(CMake, ["cmake"]),
    entry!(
        Cpp,
        ["cpp", "c++", "cc", "cxx", "hh", "hpp", "hxx"],
        &CPP,
        Some("//"),
        Disabled,
        None
    ),
    plain!(CSharp, ["csharp", "c#", "cs"]),
    entry!(Css, ["css", "scss"], &CSS, Some("/*"), Disabled, None),
    entry!(Csv, ["csv"], &CSV, None, QuotedValues, None),
    entry!(
        Cuda,
        ["cuda", "cu", "cuh"],
        &CUDA,
        Some("//"),
        Disabled,
        None
    ),
    plain!(Dart, ["dart"]),
    plain!(Elixir, ["elixir", "ex", "exs"]),
    plain!(Elm, ["elm"]),
    plain!(Erlang, ["erlang", "erl"]),
    plain!(FSharp, ["fsharp", "f#", "fs"]),
    plain!(Gleam, ["gleam"]),
    entry!(Go, ["go", "golang"], &GO, Some("//"), Disabled, None),
    plain!(Graphql, ["graphql", "gql"]),
    plain!(Groovy, ["groovy"]),
    plain!(Haskell, ["haskell", "hs"]),
    plain!(Haxe, ["haxe", "hx"]),
    entry!(Hlsl, ["hlsl"], &HLSL, Some("//"), Disabled, None),
    entry!(Html, ["html", "htm"], &HTML, None, Markup, None),
    entry!(Ini, ["ini"], &INI, Some(";"), QuotedValues, None),
    entry!(Java, ["java"], &JAVA, Some("//"), Disabled, None),
    entry!(
        JavaScript,
        ["javascript", "js", "mjs", "cjs"],
        &JAVASCRIPT,
        Some("//"),
        Disabled,
        None
    ),
    entry!(Jsx, ["jsx"], &JAVASCRIPT, Some("//"), Disabled, None),
    entry!(
        Json,
        ["json", "jsonc", "json5"],
        &JSON,
        Some("//"),
        QuotedValues,
        None
    ),
    plain!(Julia, ["julia", "jl"]),
    entry!(
        Kotlin,
        ["kotlin", "kt", "kts", "ktm"],
        &KOTLIN,
        Some("//"),
        Disabled,
        None
    ),
    plain!(Lua, ["lua"]),
    entry!(
        Make,
        ["make", "mk", "makefile"],
        &MAKE,
        Some("#"),
        Disabled,
        None
    ),
    entry!(
        Markdown,
        ["markdown", "md", "mdown", "mkd"],
        &MARKDOWN,
        None,
        Markup,
        Markdown
    ),
    entry!(
        MarkdownInline,
        ["markdown_inline", "markdown-inline"],
        &MARKDOWN_INLINE,
        None,
        Markup,
        None
    ),
    plain!(Matlab, ["matlab", "m"]),
    plain!(Nim, ["nim"]),
    plain!(ObjectiveC, ["objective-c", "objc"]),
    plain!(Ocaml, ["ocaml", "ml"]),
    plain!(Perl, ["perl", "pl"]),
    plain!(Php, ["php"]),
    entry!(
        PowerShell,
        ["powershell", "ps1", "psm1", "psd1"],
        &POWERSHELL,
        Some("#"),
        Disabled,
        None
    ),
    entry!(
        Python,
        ["python", "py", "pyw"],
        &PYTHON,
        Some("#"),
        Disabled,
        None
    ),
    plain!(R, ["r"]),
    entry!(Rst, ["rst", "rest"], &RST, None, Markup, None),
    entry!(Ruby, ["ruby", "rb"], &RUBY, Some("#"), Disabled, None),
    LanguageSupport {
        id: LanguageId::Rust,
        aliases: &["rust", "rs"],
        syntax: &RUST,
        newline: &RUST_NEWLINE,
        completion: Some(&RUST_COMPLETION),
        line_comment: Some("//"),
        spellcheck: SpellcheckMode::Disabled,
        lint: LintKind::None,
    },
    plain!(Scala, ["scala"]),
    entry!(
        Scheme,
        ["scheme", "scm"],
        &SCHEME,
        Some(";"),
        Disabled,
        None
    ),
    entry!(Slang, ["slang"], &SLANG, Some("//"), Disabled, None),
    plain!(Solidity, ["solidity", "sol"]),
    plain!(Svelte, ["svelte"]),
    plain!(Swift, ["swift"]),
    plain!(Terraform, ["terraform", "tf"]),
    entry!(Toml, ["toml"], &TOML, Some("#"), QuotedValues, None),
    entry!(
        TypeScript,
        ["typescript", "ts"],
        &TYPESCRIPT,
        Some("//"),
        Disabled,
        None
    ),
    entry!(Tsx, ["tsx"], &TSX, Some("//"), Disabled, None),
    plain!(Vala, ["vala"]),
    plain!(VisualBasic, ["visual-basic", "vb"]),
    plain!(Vue, ["vue"]),
    entry!(Xml, ["xml", "xhtml", "svg"], &XML, None, Markup, None),
    entry!(Yaml, ["yaml", "yml"], &YAML, Some("#"), QuotedValues, None),
];

pub fn language_support(name: &str) -> &'static LanguageSupport {
    let normalized = name
        .trim()
        .trim_start_matches('.')
        .split_whitespace()
        .next()
        .unwrap_or_default();
    LANGUAGES
        .iter()
        .find(|support| {
            support
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(normalized))
        })
        .unwrap_or(&LANGUAGES[0])
}

pub fn language_support_for_id(id: LanguageId) -> &'static LanguageSupport {
    LANGUAGES
        .iter()
        .find(|support| support.id == id)
        .unwrap_or(&LANGUAGES[0])
}
