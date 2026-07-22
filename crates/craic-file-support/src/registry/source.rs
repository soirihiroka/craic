use crate::{
    ContentKind, ExtensionResolver, FileSupportPatch as P, FileSupportResolver, LanguageId as L,
};

macro_rules! rule {
    ($ext:expr,$lang:expr,$mime:expr,$icon:expr) => {
        &ExtensionResolver {
            extensions: $ext,
            patch: P {
                language: Some($lang),
                content_kind: Some(ContentKind::Text),
                mime: Some($mime),
                icon_name: Some($icon),
                display_name: Some("Source code"),
                role: crate::RolePatch::Keep,
            },
        }
    };
}
pub(crate) static RESOLVERS: &[&dyn FileSupportResolver] = &[
    &ExtensionResolver {
        extensions: &["lock"],
        patch: P {
            language: Some(L::PlainText),
            content_kind: Some(ContentKind::Text),
            mime: Some("text/plain"),
            icon_name: Some("padlock2-symbolic"),
            display_name: Some("Text"),
            role: crate::RolePatch::Keep,
        },
    },
    rule!(
        &["bash", "sh", "zsh"],
        L::Bash,
        "text/x-shellscript",
        "text-x-script-symbolic"
    ),
    rule!(&["c"], L::C, "text/x-csrc", "text-x-csrc-symbolic"),
    rule!(&["h"], L::C, "text/x-chdr", "text-x-chdr-symbolic"),
    rule!(
        &["cc", "cpp", "cxx", "hh", "hpp", "hxx"],
        L::Cpp,
        "text/x-c++src",
        "text-x-c++src-symbolic"
    ),
    rule!(
        &["css", "scss", "sass"],
        L::Css,
        "text/css",
        "text-css-symbolic"
    ),
    rule!(
        &["js", "cjs", "mjs"],
        L::JavaScript,
        "text/javascript",
        "text-x-javascript-symbolic"
    ),
    rule!(&["jsx"], L::Jsx, "text/javascript", "text-x-jsx-symbolic"),
    rule!(
        &["json", "jsonc"],
        L::Json,
        "application/json",
        "text-x-javascript-symbolic"
    ),
    rule!(
        &["jsonl", "ndjson"],
        L::Json,
        "application/x-ndjson",
        "text-x-javascript-symbolic"
    ),
    rule!(&["rs"], L::Rust, "text/x-rust", "text-rust-symbolic"),
    rule!(&["toml"], L::Toml, "text/x-toml", "text-x-toml-symbolic"),
    rule!(
        &["ts"],
        L::TypeScript,
        "text/typescript",
        "text-x-javascript-symbolic"
    ),
    rule!(&["tsx"], L::Tsx, "text/typescript", "text-x-jsx-symbolic"),
    rule!(
        &["yaml", "yml"],
        L::Yaml,
        "application/x-yaml",
        "devicon-yaml-symbolic"
    ),
    rule!(&["xml"], L::Xml, "application/xml", "text-xml-symbolic"),
    rule!(&["csv"], L::Csv, "text/csv", "text-x-generic-symbolic"),
    rule!(
        &["py", "pyw"],
        L::Python,
        "text/x-python",
        "text-x-python-symbolic"
    ),
    rule!(&["rb"], L::Ruby, "text/x-ruby", "text-x-ruby-symbolic"),
    rule!(&["go"], L::Go, "text/x-go", "text-x-go-symbolic"),
    rule!(&["java"], L::Java, "text/x-java", "devicon-java-symbolic"),
    rule!(
        &["kt", "kts"],
        L::Kotlin,
        "text/x-kotlin",
        "text-x-script-symbolic"
    ),
    rule!(
        &["ps1", "psd1", "psm1"],
        L::PowerShell,
        "text/x-powershell",
        "devicon-powershell-symbolic"
    ),
    rule!(
        &["cmake"],
        L::CMake,
        "text/x-cmake",
        "text-makefile-symbolic"
    ),
    rule!(
        &["make", "mk"],
        L::Make,
        "text/x-makefile",
        "text-makefile-symbolic"
    ),
    rule!(&["ini"], L::Ini, "text/plain", "text-x-generic-symbolic"),
    rule!(
        &["scm", "ss"],
        L::Scheme,
        "text/x-scheme",
        "text-x-generic-symbolic"
    ),
    rule!(
        &["cu", "cuh"],
        L::Cuda,
        "text/x-cuda",
        "text-x-c++src-symbolic"
    ),
    rule!(&["hlsl"], L::Hlsl, "text/x-hlsl", "text-x-generic-symbolic"),
    rule!(
        &["slang"],
        L::Slang,
        "text/x-slang",
        "text-x-generic-symbolic"
    ),
    rule!(
        &["astro"],
        L::Astro,
        "text/x-astro",
        "devicon-astro-symbolic"
    ),
    rule!(
        &["bazel", "bzl"],
        L::Bazel,
        "text/x-bazel",
        "devicon-bazel-symbolic"
    ),
    rule!(
        &["cs", "csx"],
        L::CSharp,
        "text/x-csharp",
        "devicon-csharp-symbolic"
    ),
    rule!(&["dart"], L::Dart, "text/x-dart", "devicon-dart-symbolic"),
    rule!(
        &["ex", "exs"],
        L::Elixir,
        "text/x-elixir",
        "devicon-elixir-symbolic"
    ),
    rule!(&["elm"], L::Elm, "text/x-elm", "devicon-elm-symbolic"),
    rule!(
        &["erl", "hrl"],
        L::Erlang,
        "text/x-erlang",
        "devicon-erlang-symbolic"
    ),
    rule!(
        &["fs", "fsi", "fsx"],
        L::FSharp,
        "text/x-fsharp",
        "devicon-fsharp-symbolic"
    ),
    rule!(
        &["gleam"],
        L::Gleam,
        "text/x-gleam",
        "devicon-gleam-symbolic"
    ),
    rule!(
        &["gql", "graphql"],
        L::Graphql,
        "application/graphql",
        "devicon-graphql-symbolic"
    ),
    rule!(
        &["gradle", "groovy", "gvy"],
        L::Groovy,
        "text/x-groovy",
        "devicon-groovy-symbolic"
    ),
    rule!(
        &["hs", "lhs"],
        L::Haskell,
        "text/x-haskell",
        "devicon-haskell-symbolic"
    ),
    rule!(
        &["hx", "hxml"],
        L::Haxe,
        "text/x-haxe",
        "devicon-haxe-symbolic"
    ),
    rule!(&["jl"], L::Julia, "text/x-julia", "devicon-julia-symbolic"),
    rule!(&["lua"], L::Lua, "text/x-lua", "devicon-lua-symbolic"),
    rule!(
        &["mat", "mlx"],
        L::Matlab,
        "text/x-matlab",
        "devicon-matlab-symbolic"
    ),
    rule!(
        &["nim", "nims"],
        L::Nim,
        "text/x-nim",
        "devicon-nim-symbolic"
    ),
    rule!(
        &["m", "mm"],
        L::ObjectiveC,
        "text/x-objective-c",
        "devicon-objectivec-symbolic"
    ),
    rule!(
        &["ml", "mli"],
        L::Ocaml,
        "text/x-ocaml",
        "devicon-ocaml-symbolic"
    ),
    rule!(
        &["pl", "pm"],
        L::Perl,
        "text/x-perl",
        "devicon-perl-symbolic"
    ),
    rule!(
        &["php", "phtml"],
        L::Php,
        "application/x-php",
        "application-x-php-symbolic"
    ),
    rule!(&["r", "rmd"], L::R, "text/x-r", "devicon-r-symbolic"),
    rule!(
        &["scala", "sc"],
        L::Scala,
        "text/x-scala",
        "devicon-scala-symbolic"
    ),
    rule!(
        &["sol"],
        L::Solidity,
        "text/x-solidity",
        "devicon-solidity-symbolic"
    ),
    rule!(
        &["svelte"],
        L::Svelte,
        "text/x-svelte",
        "devicon-svelte-symbolic"
    ),
    rule!(&["swift"], L::Swift, "text/x-swift", "text-swift-symbolic"),
    rule!(
        &["tf", "tfvars"],
        L::Terraform,
        "text/x-terraform",
        "devicon-terraform-symbolic"
    ),
    rule!(
        &["vala", "vapi"],
        L::Vala,
        "text/x-vala",
        "text-x-vala-symbolic"
    ),
    rule!(
        &["vb", "vbs"],
        L::VisualBasic,
        "text/x-visual-basic",
        "devicon-visualbasic-symbolic"
    ),
    rule!(&["vue"], L::Vue, "text/x-vue", "devicon-vuejs-symbolic"),
];
