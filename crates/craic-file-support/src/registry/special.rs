use crate::{
    ContainsWithExtensionResolver, ContentKind as C, ExactNameResolver, ExtensionResolver,
    FileRole, FileSupportPatch as P, FileSupportResolver, LanguageId as L, MagicPrefixResolver,
    NamePrefixResolver, NameSuffixResolver,
};
pub(crate) static RESOLVERS: &[&dyn FileSupportResolver] = &[
    &ExtensionResolver {
        extensions: &["dockerfile"],
        patch: P {
            language: Some(L::Bash),
            mime: Some("text/x-dockerfile"),
            icon_name: Some("ui-container-docker-symbolic"),
            role: crate::RolePatch::Replace(Some(FileRole::Dockerfile)),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["json", "jsonc"],
        patch: P {
            display_name: Some("JSON"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["jsonl", "ndjson"],
        patch: P {
            display_name: Some("JSON Lines"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["xml"],
        patch: P {
            display_name: Some("XML"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["yaml", "yml"],
        patch: P {
            display_name: Some("YAML"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["css", "scss", "sass"],
        patch: P {
            display_name: Some("CSS"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["js", "cjs", "mjs", "jsx"],
        patch: P {
            display_name: Some("JavaScript"),
            ..P::DEFAULT
        },
    },
    &ExtensionResolver {
        extensions: &["ts", "tsx"],
        patch: P {
            display_name: Some("TypeScript"),
            ..P::DEFAULT
        },
    },
    &NamePrefixResolver {
        prefixes: &[".env."],
        patch: P {
            language: Some(L::Bash),
            icon_name: Some("text-x-script-symbolic"),
            ..P::DEFAULT
        },
    },
    &NameSuffixResolver {
        suffixes: &["ignore"],
        patch: P {
            language: Some(L::Bash),
            ..P::DEFAULT
        },
    },
    &NameSuffixResolver {
        suffixes: &[".caddyfile"],
        patch: P {
            language: Some(L::Caddy),
            ..P::DEFAULT
        },
    },
    &NameSuffixResolver {
        suffixes: &[".desktop.in"],
        patch: P {
            language: Some(L::Ini),
            ..P::DEFAULT
        },
    },
    &NamePrefixResolver {
        prefixes: &["dockerfile."],
        patch: P {
            language: Some(L::Bash),
            content_kind: Some(C::Text),
            mime: Some("text/x-dockerfile"),
            icon_name: Some("ui-container-docker-symbolic"),
            display_name: Some("Source code"),
            role: crate::RolePatch::Replace(Some(FileRole::Dockerfile)),
        },
    },
    &ContainsWithExtensionResolver {
        fragment: "docker-compose",
        extensions: &["yml", "yaml"],
        patch: P {
            icon_name: Some("ui-container-docker-symbolic"),
            role: crate::RolePatch::Replace(Some(FileRole::Compose)),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["compose.yml", "compose.yaml"],
        patch: P {
            icon_name: Some("ui-container-docker-symbolic"),
            role: crate::RolePatch::Replace(Some(FileRole::Compose)),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["dockerfile", "containerfile", ".dockerignore"],
        patch: P {
            language: Some(L::Bash),
            content_kind: Some(C::Text),
            mime: Some("text/x-dockerfile"),
            icon_name: Some("ui-container-docker-symbolic"),
            display_name: Some("Source code"),
            role: crate::RolePatch::Replace(Some(FileRole::Dockerfile)),
        },
    },
    &ExactNameResolver {
        names: &["cargo.lock"],
        patch: P {
            language: Some(L::Toml),
            icon_name: Some("text-rust-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["uv.lock"],
        patch: P {
            language: Some(L::Toml),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["cargo.toml", "pyproject.toml", "bunfig.toml"],
        patch: P {
            language: Some(L::Toml),
            mime: Some("text/x-toml"),
            icon_name: Some("text-x-toml-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["bun.lock"],
        patch: P {
            icon_name: Some("devicon-bun-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["bun.lockb"],
        patch: P {
            icon_name: Some("devicon-bun-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["makefile", "gnumakefile"],
        patch: P {
            language: Some(L::Make),
            mime: Some("text/x-makefile"),
            icon_name: Some("text-makefile-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[
            ".bash_profile",
            ".bashrc",
            ".profile",
            ".zprofile",
            ".zshrc",
        ],
        patch: P {
            language: Some(L::Bash),
            icon_name: Some("text-x-script-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[".env"],
        patch: P {
            language: Some(L::Bash),
            icon_name: Some("text-x-script-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["caddyfile"],
        patch: P {
            language: Some(L::Caddy),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["gemfile"],
        patch: P {
            language: Some(L::Ruby),
            icon_name: Some("devicon-ruby-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["rakefile"],
        patch: P {
            language: Some(L::Ruby),
            mime: Some("text/x-makefile"),
            icon_name: Some("devicon-ruby-symbolic"),
            display_name: Some("Source code"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["package.json", "composer.json", "tsconfig.json"],
        patch: P {
            language: Some(L::Json),
            mime: Some("application/json"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"],
        patch: P {
            mime: Some("text/plain"),
            icon_name: Some("text-makefile-symbolic"),
            display_name: Some("Text"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["composer.lock"],
        patch: P {
            mime: Some("text/plain"),
            icon_name: Some("devicon-php-symbolic"),
            display_name: Some("Text"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["mix.lock"],
        patch: P {
            mime: Some("text/plain"),
            icon_name: Some("devicon-elixir-symbolic"),
            display_name: Some("Text"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["mix.exs"],
        patch: P {
            language: Some(L::Elixir),
            mime: Some("text/x-elixir"),
            icon_name: Some("devicon-elixir-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["cmakelists.txt"],
        patch: P {
            language: Some(L::CMake),
            mime: Some("text/x-cmake"),
            icon_name: Some("text-makefile-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["go.mod", "go.sum", "go.work"],
        patch: P {
            mime: Some("text/plain"),
            icon_name: Some("text-makefile-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["package.swift"],
        patch: P {
            language: Some(L::Swift),
            icon_name: Some("text-swift-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["pom.xml"],
        patch: P {
            language: Some(L::Xml),
            mime: Some("application/xml"),
            icon_name: Some("text-makefile-symbolic"),
            display_name: Some("XML"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["pubspec.yaml", "pubspec.yml"],
        patch: P {
            language: Some(L::Yaml),
            mime: Some("application/x-yaml"),
            icon_name: Some("devicon-dart-symbolic"),
            display_name: Some("YAML"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["sketch.yaml", "sketch.yml"],
        patch: P {
            language: Some(L::Yaml),
            mime: Some("application/x-yaml"),
            icon_name: Some("text-makefile-symbolic"),
            display_name: Some("YAML"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["build.gradle", "build.gradle.kts"],
        patch: P {
            icon_name: Some("devicon-gradle-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["requirements.txt", "wscript"],
        patch: P {
            icon_name: Some("text-makefile-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[".node-version", ".nvmrc"],
        patch: P {
            icon_name: Some("devicon-nodejs-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[".python-version"],
        patch: P {
            icon_name: Some("text-x-python-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[
            ".gitattributes",
            ".git-blame-ignore-revs",
            ".gitignore",
            ".gitkeep",
            ".gitmodules",
        ],
        patch: P {
            icon_name: Some("devicon-git-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[
            "readme",
            "readme.md",
            "readme.markdown",
            "readme.rest",
            "readme.rst",
            "readme.txt",
        ],
        patch: P {
            icon_name: Some("text-x-readme-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["changelog", "news"],
        patch: P {
            icon_name: Some("text-x-changelog-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["copying"],
        patch: P {
            icon_name: Some("text-x-copying-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &["authors", "maintainers"],
        patch: P {
            icon_name: Some("text-x-authors-symbolic"),
            ..P::DEFAULT
        },
    },
    &ExactNameResolver {
        names: &[
            "license",
            "license.md",
            "license.txt",
            "licence",
            "licence.md",
            "licence.txt",
        ],
        patch: P {
            icon_name: Some("license-symbolic"),
            ..P::DEFAULT
        },
    },
    &MagicPrefixResolver {
        prefix: b"SQLite format 3\0",
        patch: P {
            language: Some(L::PlainText),
            content_kind: Some(C::Sqlite),
            mime: Some("application/vnd.sqlite3"),
            icon_name: Some("text-sql-symbolic"),
            display_name: Some("SQLite database"),
            role: crate::RolePatch::Replace(None),
        },
    },
];

impl P {
    pub(crate) const DEFAULT: Self = Self {
        language: None,
        content_kind: None,
        mime: None,
        icon_name: None,
        display_name: None,
        role: crate::RolePatch::Keep,
    };
}
