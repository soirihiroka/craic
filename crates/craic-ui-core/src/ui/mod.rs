pub mod canvas_painter;
pub mod canvas_scroll;
pub mod canvas_scrollbar;
pub mod command_mailbox;
pub mod components;
pub mod dialogs;
pub mod file_row;
pub mod file_status;
pub mod file_type;
pub mod left_clamp;
pub mod pages;
pub mod picker;
pub mod widgets;

use std::path::PathBuf;

pub const PAN_DOWN_SYMBOLIC: &[u8] = include_bytes!("../../assets/icons/pan-down-symbolic.svg");

pub fn asset_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons")];
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data_home).join("craic/assets"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".local/share/craic/assets"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    paths.extend(
        data_dirs
            .split(':')
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(path).join("craic/assets")),
    );
    paths
}

pub fn bundled_font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/JetBrainsMono")
}
