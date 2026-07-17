use gtk::gio;
use std::path::{Path, PathBuf};

pub const MIME_FOLDER: &str = "inode/directory";
pub const MIME_MARKDOWN: &str = "text/markdown";
pub const MIME_PDF: &str = "application/pdf";
pub const MIME_SQLITE: &str = "application/vnd.sqlite3";
pub const MIME_SVG: &str = "image/svg+xml";
pub const MIME_SAFETENSORS: &str = "application/vnd.safetensors";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Folder,
    Text,
    Notebook,
    Markdown,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileType {
    pub mime: &'static str,
    pub icon_name: &'static str,
    pub display_kind: &'static str,
}

pub fn detect(path: &str, is_dir: bool) -> FileType {
    if is_dir {
        return FileType {
            mime: MIME_FOLDER,
            icon_name: "folder-symbolic",
            display_kind: "Folder",
        };
    }

    let file_name = base_name(path).to_ascii_lowercase();
    let mime_extension = Path::new(&file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let icon_extension = file_name.rsplit('.').next().unwrap_or_default();
    let mime = mime_from_name_extension(&file_name, mime_extension);

    FileType {
        mime,
        icon_name: icon_name_from_name_extension(&file_name, icon_extension),
        display_kind: display_kind_for_mime(mime),
    }
}

pub fn icon(file_type: &FileType) -> gtk::Image {
    icon_for_name(file_type.icon_name)
}

pub fn icon_for_name(icon_name: &str) -> gtk::Image {
    if let Some(icon_path) = bundled_icon_path(icon_name) {
        let file = gio::File::for_path(icon_path);
        let paintable = gtk::IconPaintable::for_file(&file, 16, 1);
        return gtk::Image::from_paintable(Some(&paintable));
    }

    gtk::Image::from_icon_name(icon_name)
}

pub fn set_icon_for_name(image: &gtk::Image, icon_name: &str) {
    if let Some(icon_path) = bundled_icon_path(icon_name) {
        let file = gio::File::for_path(icon_path);
        let paintable = gtk::IconPaintable::for_file(&file, 16, 1);
        image.set_paintable(Some(&paintable));
        return;
    }

    image.set_icon_name(Some(icon_name));
}

pub fn preview_kind_for_path(path: &str, is_dir: bool) -> PreviewKind {
    preview_kind(&detect(path, is_dir))
}

fn preview_kind(file_type: &FileType) -> PreviewKind {
    if file_type.mime == MIME_FOLDER {
        return PreviewKind::Folder;
    }
    if file_type.mime == "application/x-ipynb+json" {
        return PreviewKind::Notebook;
    }
    if file_type.mime == MIME_MARKDOWN {
        return PreviewKind::Markdown;
    }
    if file_type.mime == "text/html" {
        return PreviewKind::Html;
    }
    if file_type.mime == MIME_SVG {
        return PreviewKind::Svg;
    }
    if file_type.mime == MIME_PDF {
        return PreviewKind::Pdf;
    }
    if file_type.mime == MIME_SQLITE {
        return PreviewKind::Sqlite;
    }
    if file_type.mime == MIME_SAFETENSORS {
        return PreviewKind::Safetensors;
    }
    if file_type.mime == "application/vnd.ms-fontobject" || file_type.mime.starts_with("font/") {
        return PreviewKind::Font;
    }
    if file_type.mime.starts_with("image/") {
        return PreviewKind::Image;
    }
    if file_type.mime.starts_with("audio/") {
        return PreviewKind::Audio;
    }
    if file_type.mime.starts_with("video/") {
        return PreviewKind::Video;
    }

    PreviewKind::Text
}

pub fn is_sqlite_database_name(path: &str) -> bool {
    let file_name = base_name(path).to_ascii_lowercase();
    Path::new(&file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "db" | "sqlite" | "sqlite3"))
}

fn base_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

fn mime_from_name_extension(file_name: &str, extension: &str) -> &'static str {
    if file_name == "dockerfile" || file_name.starts_with("dockerfile.") {
        return "text/x-dockerfile";
    }

    match file_name {
        ".dockerignore" | "containerfile" => return "text/x-dockerfile",
        name if is_compose_file_name(name) => return "application/x-yaml",
        "cargo.toml" | "pyproject.toml" | "bunfig.toml" => return "text/x-toml",
        name if name == ".env" || name.starts_with(".env.") => return "text/plain",
        "cargo.lock" | "bun.lock" | "composer.lock" | "mix.lock" | "package-lock.json"
        | "pnpm-lock.yaml" | "yarn.lock" => return "text/plain",
        "cmakelists.txt" => return "text/x-cmake",
        "gnumakefile" | "makefile" | "rakefile" => return "text/x-makefile",
        "go.mod" | "go.sum" | "go.work" => return "text/plain",
        "package.json" | "composer.json" | "tsconfig.json" => return "application/json",
        name if name.ends_with(".ipynb") => return "application/x-ipynb+json",
        "pom.xml" => return "application/xml",
        "pubspec.yaml" | "pubspec.yml" | "sketch.yaml" | "sketch.yml" => {
            return "application/x-yaml";
        }
        _ => {}
    }

    match extension {
        "md" | "mdown" | "mdx" | "mkd" | "markdown" => MIME_MARKDOWN,
        "db" | "sqlite" | "sqlite3" => MIME_SQLITE,
        "safetensors" => MIME_SAFETENSORS,
        "svg" | "svgz" => MIME_SVG,
        "apng" => "image/apng",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "dds" => "image/vnd.ms-dds",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "jp2" => "image/jp2",
        "ico" => "image/vnd.microsoft.icon",
        "jpeg" | "jpg" => "image/jpeg",
        "jxl" => "image/jxl",
        "pbm" => "image/x-portable-bitmap",
        "pgm" => "image/x-portable-graymap",
        "ppm" => "image/x-portable-pixmap",
        "pnm" => "image/x-portable-anymap",
        "png" => "image/png",
        "qoi" => "image/qoi",
        "tif" | "tiff" => "image/tiff",
        "tga" => "image/x-tga",
        "webp" => "image/webp",
        "xbm" => "image/x-xbitmap",
        "xpm" => "image/x-xpixmap",
        "dng" => "image/x-adobe-dng",
        "cr2" => "image/x-canon-cr2",
        "erf" => "image/x-epson-erf",
        "exr" => "image/exr",
        "mrw" => "image/x-minolta-mrw",
        "orf" => "image/x-olympus-orf",
        "pef" => "image/x-pentax-pef",
        "raw" => "image/x-raw",
        "srf" => "image/x-sony-srf",
        "aac" => "audio/aac",
        "aif" | "aifc" | "aiff" => "audio/aiff",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mid" | "midi" => "audio/midi",
        "mp3" => "audio/mpeg",
        "oga" | "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "wma" => "audio/x-ms-wma",
        "pdf" => MIME_PDF,
        "3gp" => "video/3gpp",
        "avi" => "video/x-msvideo",
        "flv" => "video/x-flv",
        "m4v" => "video/x-m4v",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp4" => "video/mp4",
        "mpeg" | "mpg" => "video/mpeg",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "wmv" => "video/x-ms-wmv",
        "eot" => "application/vnd.ms-fontobject",
        "otf" | "otc" => "font/otf",
        "ttf" | "ttc" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "astro" => "text/x-astro",
        "bash" | "sh" | "zsh" => "text/x-shellscript",
        "bazel" | "bzl" => "text/x-bazel",
        "c" => "text/x-csrc",
        "h" => "text/x-chdr",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "text/x-c++src",
        "cmake" => "text/x-cmake",
        "cs" | "csx" => "text/x-csharp",
        "css" | "scss" | "sass" => "text/css",
        "dart" => "text/x-dart",
        "dockerfile" => "text/x-dockerfile",
        "elm" => "text/x-elm",
        "erl" | "hrl" => "text/x-erlang",
        "ex" | "exs" => "text/x-elixir",
        "fs" | "fsi" | "fsx" => "text/x-fsharp",
        "gleam" => "text/x-gleam",
        "go" => "text/x-go",
        "gql" | "graphql" => "application/graphql",
        "gradle" | "groovy" | "gvy" => "text/x-groovy",
        "hs" | "lhs" => "text/x-haskell",
        "htm" | "html" | "xhtml" => "text/html",
        "hxml" | "hx" => "text/x-haxe",
        "java" => "text/x-java",
        "jl" => "text/x-julia",
        "js" | "cjs" | "mjs" | "jsx" => "text/javascript",
        "json" | "jsonc" => "application/json",
        "ipynb" => "application/x-ipynb+json",
        "kt" | "kts" => "text/x-kotlin",
        "lua" => "text/x-lua",
        "m" | "mm" => "text/x-objective-c",
        "make" | "mk" => "text/x-makefile",
        "mat" | "mlx" => "text/x-matlab",
        "ml" | "mli" => "text/x-ocaml",
        "nim" | "nims" => "text/x-nim",
        "php" | "phtml" => "application/x-php",
        "pl" | "pm" => "text/x-perl",
        "ps1" | "psd1" | "psm1" => "text/x-powershell",
        "py" | "pyw" => "text/x-python",
        "r" | "rmd" => "text/x-r",
        "rb" => "text/x-ruby",
        "rs" => "text/x-rust",
        "scala" | "sc" => "text/x-scala",
        "sol" => "text/x-solidity",
        "svelte" => "text/x-svelte",
        "swift" => "text/x-swift",
        "tf" | "tfvars" => "text/x-terraform",
        "toml" => "text/x-toml",
        "ts" | "tsx" => "text/typescript",
        "vala" | "vapi" => "text/x-vala",
        "vb" | "vbs" => "text/x-visual-basic",
        "vue" => "text/x-vue",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/x-yaml",
        _ => "text/plain",
    }
}

fn display_kind_for_mime(mime: &str) -> &'static str {
    match mime {
        MIME_FOLDER => "Folder",
        MIME_MARKDOWN => "Markdown",
        MIME_PDF => "PDF",
        MIME_SQLITE => "SQLite database",
        MIME_SVG => "SVG image",
        MIME_SAFETENSORS => "Safetensors metadata",
        "application/x-ipynb+json" => "Jupyter notebook",
        "application/json" => "JSON",
        "application/xml" => "XML",
        "application/x-yaml" => "YAML",
        "text/css" => "CSS",
        "text/html" => "HTML",
        "text/javascript" => "JavaScript",
        "text/typescript" => "TypeScript",
        mime if mime.starts_with("audio/") => "Audio",
        mime if mime.starts_with("font/") => "Font",
        mime if mime.starts_with("image/") => "Image",
        "application/vnd.ms-fontobject" => "Font",
        mime if mime.starts_with("text/x-") => "Source code",
        mime if mime.starts_with("video/") => "Video",
        _ => "Text",
    }
}

fn icon_name_from_name_extension(file_name: &str, extension: &str) -> &'static str {
    if file_name == "dockerfile" || file_name.starts_with("dockerfile.") {
        return "ui-container-docker-symbolic";
    }

    match file_name {
        ".bash_profile" | ".bashrc" | ".profile" | ".zprofile" | ".zshrc" => {
            return "text-x-script-symbolic";
        }
        ".dockerignore" => {
            return "ui-container-docker-symbolic";
        }
        name if is_compose_file_name(name) => return "ui-container-docker-symbolic",
        ".gitattributes" | ".git-blame-ignore-revs" | ".gitignore" | ".gitkeep" | ".gitmodules" => {
            return "devicon-git-symbolic";
        }
        "license" | "license.md" | "license.txt" | "licence" | "licence.md" | "licence.txt" => {
            return "license-symbolic";
        }
        "changelog" => return "text-x-changelog-symbolic",
        "copying" => return "text-x-copying-symbolic",
        "authors" | "maintainers" => return "text-x-authors-symbolic",
        "readme" | "readme.md" | "readme.markdown" | "readme.txt" => {
            return "text-x-readme-symbolic";
        }
        "news" => return "text-x-changelog-symbolic",
        name if name == ".env" || name.starts_with(".env.") => return "text-x-script-symbolic",
        ".node-version" | ".nvmrc" => return "devicon-nodejs-symbolic",
        ".python-version" => return "text-x-python-symbolic",
        "build.gradle" | "build.gradle.kts" => return "devicon-gradle-symbolic",
        "bun.lock" | "bun.lockb" => return "devicon-bun-symbolic",
        "bunfig.toml" => return "text-x-toml-symbolic",
        "cargo.lock" => return "text-rust-symbolic",
        "cargo.toml" => return "text-x-toml-symbolic",
        "cmakelists.txt" => return "text-makefile-symbolic",
        "composer.json" | "composer.lock" => return "devicon-php-symbolic",
        "go.mod" | "go.sum" | "go.work" => return "text-makefile-symbolic",
        "gemfile" | "rakefile" => return "devicon-ruby-symbolic",
        "gnumakefile" | "makefile" => return "text-makefile-symbolic",
        "mix.exs" | "mix.lock" => return "devicon-elixir-symbolic",
        "package.json" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" => {
            return "text-makefile-symbolic";
        }
        "package.swift" => return "text-swift-symbolic",
        "pom.xml" => return "text-makefile-symbolic",
        "pubspec.yaml" | "pubspec.yml" => return "devicon-dart-symbolic",
        "pyproject.toml" => return "text-x-toml-symbolic",
        "requirements.txt" => return "text-makefile-symbolic",
        "containerfile" | "wscript" => return "text-makefile-symbolic",
        "sketch.yaml" | "sketch.yml" => return "text-makefile-symbolic",
        "tsconfig.json" => return "text-x-script-symbolic",
        _ => {}
    }

    match extension {
        "env" => "text-x-script-symbolic",
        "toml" => "text-x-toml-symbolic",
        "astro" => "devicon-astro-symbolic",
        "bash" | "sh" | "zsh" => "text-x-script-symbolic",
        "bazel" | "bzl" => "devicon-bazel-symbolic",
        "c" => "text-x-csrc-symbolic",
        "h" => "text-x-chdr-symbolic",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "text-x-c++src-symbolic",
        "cmake" => "text-makefile-symbolic",
        "cs" | "csx" => "devicon-csharp-symbolic",
        "css" | "scss" | "sass" => "text-css-symbolic",
        "dart" => "devicon-dart-symbolic",
        "dockerfile" => "ui-container-docker-symbolic",
        "elm" => "devicon-elm-symbolic",
        "erl" | "hrl" => "devicon-erlang-symbolic",
        "ex" | "exs" => "devicon-elixir-symbolic",
        "fs" | "fsi" | "fsx" => "devicon-fsharp-symbolic",
        "gleam" => "devicon-gleam-symbolic",
        "go" => "text-x-go-symbolic",
        "gql" | "graphql" => "devicon-graphql-symbolic",
        "gradle" | "groovy" | "gvy" => "devicon-groovy-symbolic",
        "hs" | "lhs" => "devicon-haskell-symbolic",
        "htm" | "html" | "xhtml" => "text-html-symbolic",
        "hxml" | "hx" => "devicon-haxe-symbolic",
        "java" => "devicon-java-symbolic",
        "jl" => "devicon-julia-symbolic",
        "js" | "cjs" | "mjs" => "text-x-javascript-symbolic",
        "jsx" => "text-x-jsx-symbolic",
        "json" | "jsonc" => "text-x-javascript-symbolic",
        "kt" | "kts" => "text-x-script-symbolic",
        "lua" => "devicon-lua-symbolic",
        "m" | "mm" => "devicon-objectivec-symbolic",
        "make" | "mk" => "text-makefile-symbolic",
        "md" | "markdown" | "mdown" | "mdx" | "mkd" => "text-markdown-symbolic",
        "mat" | "mlx" => "devicon-matlab-symbolic",
        "ml" | "mli" => "devicon-ocaml-symbolic",
        "nim" | "nims" => "devicon-nim-symbolic",
        "php" | "phtml" => "application-x-php-symbolic",
        "pdf" => "rich-text-symbolic",
        "db" | "sqlite" | "sqlite3" => "text-sql-symbolic",
        "pl" | "pm" => "devicon-perl-symbolic",
        "ps1" | "psd1" | "psm1" => "devicon-powershell-symbolic",
        "py" | "pyw" => "text-x-python-symbolic",
        "r" | "rmd" => "devicon-r-symbolic",
        "rb" => "text-x-ruby-symbolic",
        "rs" => "text-rust-symbolic",
        "scala" | "sc" => "devicon-scala-symbolic",
        "sol" => "devicon-solidity-symbolic",
        "svg" | "svgz" => "svg-symbolic",
        "svelte" => "devicon-svelte-symbolic",
        "swift" => "text-swift-symbolic",
        "tf" | "tfvars" => "devicon-terraform-symbolic",
        "ts" => "text-x-javascript-symbolic",
        "tsx" => "text-x-jsx-symbolic",
        "vala" | "vapi" => "text-x-vala-symbolic",
        "vb" | "vbs" => "devicon-visualbasic-symbolic",
        "vue" => "devicon-vuejs-symbolic",
        "xml" => "text-xml-symbolic",
        "yaml" | "yml" => "devicon-yaml-symbolic",
        "aac" | "aif" | "aifc" | "aiff" | "flac" | "m4a" | "mid" | "midi" | "mp3" | "oga"
        | "ogg" | "opus" | "wav" | "weba" | "wma" => "audio-x-generic-symbolic",
        "apng" | "avif" | "bmp" | "gif" | "heic" | "heif" | "ico" | "jpeg" | "jpg" | "jxl"
        | "dds" | "jp2" | "pbm" | "pgm" | "png" | "pnm" | "ppm" | "qoi" | "tga" | "tif"
        | "tiff" | "webp" | "xpm" | "xbm" | "dng" | "cr2" | "exr" | "erf" | "mrw" | "orf"
        | "pef" | "raw" | "srf" => "image-x-generic-symbolic",
        "3gp" | "avi" | "flv" | "m4v" | "mkv" | "mov" | "mp4" | "mpeg" | "mpg" | "ogv" | "webm"
        | "wmv" => "video-x-generic-symbolic",
        _ => "text-x-generic-symbolic",
    }
}

fn is_compose_file_name(name: &str) -> bool {
    matches!(name, "compose.yml" | "compose.yaml")
        || (name.contains("docker-compose") && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

fn bundled_icon_path(icon_name: &str) -> Option<PathBuf> {
    for assets_dir in crate::ui::asset_search_paths() {
        let candidate = assets_dir.join(format!("{icon_name}.svg"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}
