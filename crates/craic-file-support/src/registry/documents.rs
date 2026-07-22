use crate::{ContentKind, ExtensionResolver, FileSupportPatch, FileSupportResolver, LanguageId};

static MARKDOWN: ExtensionResolver = ExtensionResolver {
    extensions: &["md", "mdown", "mdx", "mkd", "markdown"],
    patch: FileSupportPatch {
        language: Some(LanguageId::Markdown),
        content_kind: Some(ContentKind::Markdown),
        mime: Some("text/markdown"),
        icon_name: Some("text-markdown-symbolic"),
        display_name: Some("Markdown"),
        role: crate::RolePatch::Keep,
    },
};

static RST: ExtensionResolver = ExtensionResolver {
    extensions: &["rest", "rst"],
    patch: FileSupportPatch {
        language: Some(LanguageId::Rst),
        content_kind: Some(ContentKind::Rst),
        mime: Some("text/x-rst"),
        icon_name: Some("rich-text-symbolic"),
        display_name: Some("reStructuredText"),
        role: crate::RolePatch::Keep,
    },
};

static HTML: ExtensionResolver = ExtensionResolver {
    extensions: &["htm", "html", "xhtml"],
    patch: FileSupportPatch {
        language: Some(LanguageId::Html),
        content_kind: Some(ContentKind::Html),
        mime: Some("text/html"),
        icon_name: Some("text-html-symbolic"),
        display_name: Some("HTML"),
        role: crate::RolePatch::Keep,
    },
};

static SVG: ExtensionResolver = ExtensionResolver {
    extensions: &["svg", "svgz"],
    patch: FileSupportPatch {
        language: Some(LanguageId::Xml),
        content_kind: Some(ContentKind::Svg),
        mime: Some("image/svg+xml"),
        icon_name: Some("svg-symbolic"),
        display_name: Some("SVG image"),
        role: crate::RolePatch::Keep,
    },
};

static NOTEBOOK: ExtensionResolver = ExtensionResolver {
    extensions: &["ipynb"],
    patch: FileSupportPatch {
        language: Some(LanguageId::Json),
        content_kind: Some(ContentKind::Notebook),
        mime: Some("application/x-ipynb+json"),
        icon_name: Some("text-x-javascript-symbolic"),
        display_name: Some("Jupyter notebook"),
        role: crate::RolePatch::Keep,
    },
};

static PDF: ExtensionResolver = ExtensionResolver {
    extensions: &["pdf"],
    patch: FileSupportPatch {
        language: Some(LanguageId::PlainText),
        content_kind: Some(ContentKind::Pdf),
        mime: Some("application/pdf"),
        icon_name: Some("rich-text-symbolic"),
        display_name: Some("PDF"),
        role: crate::RolePatch::Keep,
    },
};

static SAFETENSORS: ExtensionResolver = ExtensionResolver {
    extensions: &["safetensors"],
    patch: FileSupportPatch {
        language: Some(LanguageId::PlainText),
        content_kind: Some(ContentKind::Safetensors),
        mime: Some("application/vnd.safetensors"),
        icon_name: Some("text-x-generic-symbolic"),
        display_name: Some("Safetensors metadata"),
        role: crate::RolePatch::Keep,
    },
};

static SQLITE: ExtensionResolver = ExtensionResolver {
    extensions: &["db", "sqlite", "sqlite3"],
    patch: FileSupportPatch {
        language: Some(LanguageId::PlainText),
        content_kind: Some(ContentKind::Sqlite),
        mime: Some("application/vnd.sqlite3"),
        icon_name: Some("text-sql-symbolic"),
        display_name: Some("SQLite database"),
        role: crate::RolePatch::Keep,
    },
};

pub(crate) static RESOLVERS: &[&dyn FileSupportResolver] = &[
    &MARKDOWN,
    &RST,
    &HTML,
    &SVG,
    &NOTEBOOK,
    &PDF,
    &SAFETENSORS,
    &SQLITE,
];
