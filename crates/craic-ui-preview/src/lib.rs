pub use craic_language as language_support;
pub mod csv_table;
pub mod markdown_preview_web;
pub mod notebook_preview_web;
pub mod safetensors_metadata;

#[cfg(target_os = "linux")]
pub use craic_vcs::git;

#[cfg(target_os = "linux")]
pub mod markdown_preview;
#[cfg(target_os = "linux")]
pub mod ui;

#[cfg(target_os = "linux")]
pub use ui::content::{binary_preview, folder_view, pdf_preview};
