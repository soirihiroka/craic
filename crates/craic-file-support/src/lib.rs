mod matcher;
mod registry;

pub use matcher::{
    ContainsWithExtensionResolver, DefaultResolver, ExactNameResolver, ExtensionResolver,
    FileSupportMatch, FileSupportResolver, MagicPrefixResolver, MatchLevel, NamePrefixResolver,
    NameSuffixResolver, NormalizedFileProbe, resolve,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LanguageId {
    PlainText,
    Astro,
    Bash,
    Bazel,
    C,
    Caddy,
    CMake,
    Cpp,
    CSharp,
    Css,
    Csv,
    Cuda,
    Dart,
    Elixir,
    Elm,
    Erlang,
    FSharp,
    Gleam,
    Go,
    Graphql,
    Groovy,
    Haskell,
    Haxe,
    Hlsl,
    Html,
    Ini,
    Java,
    JavaScript,
    Jsx,
    Json,
    Julia,
    Kotlin,
    Lua,
    Make,
    Markdown,
    MarkdownInline,
    Matlab,
    Nim,
    ObjectiveC,
    Ocaml,
    Perl,
    Php,
    PowerShell,
    Python,
    R,
    Rst,
    Ruby,
    Rust,
    Scala,
    Scheme,
    Slang,
    Solidity,
    Svelte,
    Swift,
    Terraform,
    Toml,
    TypeScript,
    Tsx,
    Vala,
    VisualBasic,
    Vue,
    Xml,
    Yaml,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContentKind {
    Folder,
    Text,
    Notebook,
    Markdown,
    Rst,
    Html,
    Svg,
    Safetensors,
    Image,
    Audio,
    Video,
    Font,
    Pdf,
    Sqlite,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileRole {
    Dockerfile,
    Compose,
}

#[derive(Clone, Copy, Debug)]
pub struct FileProbe<'a> {
    pub path: &'a str,
    pub is_dir: bool,
    pub leading_bytes: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResolvedFileSupport {
    pub language: LanguageId,
    pub content_kind: ContentKind,
    pub mime: &'static str,
    pub icon_name: &'static str,
    pub display_name: &'static str,
    pub role: Option<FileRole>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FileSupportPatch {
    pub language: Option<LanguageId>,
    pub content_kind: Option<ContentKind>,
    pub mime: Option<&'static str>,
    pub icon_name: Option<&'static str>,
    pub display_name: Option<&'static str>,
    pub role: RolePatch,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum RolePatch {
    #[default]
    Keep,
    Replace(Option<FileRole>),
}
