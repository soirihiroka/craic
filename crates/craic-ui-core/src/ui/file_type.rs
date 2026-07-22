use gtk::gio;
use std::path::PathBuf;

pub use craic_file_support::{ContentKind as PreviewKind, FileRole, LanguageId};
use craic_file_support::{FileProbe, ResolvedFileSupport, resolve};

pub const MIME_FOLDER: &str = "inode/directory";
pub const MIME_JSON_LINES: &str = "application/x-ndjson";
pub const MIME_MARKDOWN: &str = "text/markdown";
pub const MIME_PDF: &str = "application/pdf";
pub const MIME_RST: &str = "text/x-rst";
pub const MIME_SQLITE: &str = "application/vnd.sqlite3";
pub const MIME_SVG: &str = "image/svg+xml";
pub const MIME_SAFETENSORS: &str = "application/vnd.safetensors";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileType {
    pub mime: &'static str,
    pub icon_name: &'static str,
    pub display_kind: &'static str,
    pub language: LanguageId,
    pub preview_kind: PreviewKind,
    pub role: Option<FileRole>,
}

impl From<ResolvedFileSupport> for FileType {
    fn from(value: ResolvedFileSupport) -> Self {
        Self {
            mime: value.mime,
            icon_name: value.icon_name,
            display_kind: value.display_name,
            language: value.language,
            preview_kind: value.content_kind,
            role: value.role,
        }
    }
}

pub fn detect(path: &str, is_dir: bool) -> FileType {
    detect_with_bytes(path, is_dir, None)
}

pub fn detect_with_bytes(path: &str, is_dir: bool, leading_bytes: Option<&[u8]>) -> FileType {
    resolve(FileProbe {
        path,
        is_dir,
        leading_bytes,
    })
    .into()
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
    detect(path, is_dir).preview_kind
}

fn bundled_icon_path(icon_name: &str) -> Option<PathBuf> {
    crate::ui::asset_search_paths()
        .into_iter()
        .map(|dir| dir.join(format!("{icon_name}.svg")))
        .find(|candidate| candidate.is_file())
}
