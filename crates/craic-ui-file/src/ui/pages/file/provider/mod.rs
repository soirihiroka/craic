pub(super) mod csv;
mod folder;
mod font;
pub(super) mod html;
mod image_viewer;
pub mod markdown;
pub mod media;
pub mod notebook;
mod notebook_readonly;
mod pdf;
mod safetensors;
pub mod sqlite;
pub mod svg;
pub(super) mod table_view;
mod text;

use super::{PageContext, right};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileAccess, FileKind, FileNodeInfo};
use crate::ui::file_type;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

const PREVIEW_POLL_MS: u64 = 30;

pub type PreviewFn = for<'a> fn(PreviewRequest<'a>);
pub type MatchPreviewFn = for<'a> fn(PreviewMatchRequest<'a>);

#[derive(Clone, Copy)]
pub struct Provider {
    pub show: PreviewFn,
    pub show_match: MatchPreviewFn,
}

pub struct PreviewRequest<'a> {
    pub ctx: PageContext,
    pub right: Rc<right::RightPane>,
    pub load_token: right::PreviewLoadToken,
    pub files: Arc<dyn FileAccess>,
    pub file_path: &'a str,
    pub node_path: &'a FileNodePath,
    pub local_path: Option<&'a Path>,
    pub info: &'a FileNodeInfo,
    pub prefetched_bytes: Option<&'a [u8]>,
}

pub struct PreviewMatchRequest<'a> {
    pub ctx: PageContext,
    pub right: Rc<right::RightPane>,
    pub load_token: right::PreviewLoadToken,
    pub files: Arc<dyn FileAccess>,
    pub file_path: &'a str,
    pub node_path: &'a FileNodePath,
    pub local_path: Option<&'a Path>,
    pub info: &'a FileNodeInfo,
    pub prefetched_bytes: Option<&'a [u8]>,
    pub start: usize,
    pub end: usize,
}

impl<'a> PreviewMatchRequest<'a> {
    pub fn into_preview_request(self) -> PreviewRequest<'a> {
        PreviewRequest {
            ctx: self.ctx,
            right: self.right,
            load_token: self.load_token,
            files: self.files,
            file_path: self.file_path,
            node_path: self.node_path,
            local_path: self.local_path,
            info: self.info,
            prefetched_bytes: self.prefetched_bytes,
        }
    }
}

pub fn for_file(file_path: &str, info: &FileNodeInfo, prefetched_bytes: Option<&[u8]>) -> Provider {
    let is_file = info.kind.is_file();
    let is_dir = info.kind == FileKind::Directory;
    let preview_kind = file_type::detect_with_bytes(
        file_path,
        is_dir,
        is_file.then_some(prefetched_bytes).flatten(),
    )
    .preview_kind;

    if preview_kind == file_type::PreviewKind::Sqlite {
        log::debug!("sqlite preview selected file_path={file_path}");
    }

    const PROVIDERS: &[(file_type::PreviewKind, Provider)] = &[
        (
            file_type::PreviewKind::Folder,
            Provider {
                show: folder::show,
                show_match: folder::show_match,
            },
        ),
        (
            file_type::PreviewKind::Notebook,
            Provider {
                show: notebook::show,
                show_match: notebook::show_match,
            },
        ),
        (
            file_type::PreviewKind::Svg,
            Provider {
                show: svg::show,
                show_match: svg::show_match,
            },
        ),
        (
            file_type::PreviewKind::Safetensors,
            Provider {
                show: safetensors::show,
                show_match: safetensors::show_match,
            },
        ),
        (
            file_type::PreviewKind::Markdown,
            Provider {
                show: markdown::show,
                show_match: markdown::show_match,
            },
        ),
        (
            file_type::PreviewKind::Rst,
            Provider {
                show: markdown::show_rst,
                show_match: markdown::show_rst_match,
            },
        ),
        (
            file_type::PreviewKind::Html,
            Provider {
                show: html::show,
                show_match: html::show_match,
            },
        ),
        (
            file_type::PreviewKind::Image,
            Provider {
                show: media::show_image,
                show_match: media::show_image_match,
            },
        ),
        (
            file_type::PreviewKind::Audio,
            Provider {
                show: media::show_audio,
                show_match: media::show_audio_match,
            },
        ),
        (
            file_type::PreviewKind::Video,
            Provider {
                show: media::show_video,
                show_match: media::show_video_match,
            },
        ),
        (
            file_type::PreviewKind::Font,
            Provider {
                show: font::show,
                show_match: font::show_match,
            },
        ),
        (
            file_type::PreviewKind::Pdf,
            Provider {
                show: pdf::show,
                show_match: pdf::show_match,
            },
        ),
        (
            file_type::PreviewKind::Sqlite,
            Provider {
                show: sqlite::show,
                show_match: sqlite::show_match,
            },
        ),
        (
            file_type::PreviewKind::Text,
            Provider {
                show: text::show,
                show_match: text::show_match,
            },
        ),
    ];

    PROVIDERS
        .iter()
        .find_map(|(kind, provider)| (*kind == preview_kind).then_some(*provider))
        .expect("every preview kind has a provider")
}

pub fn spawn_preview_load<T, Work, Apply>(
    right: Rc<right::RightPane>,
    load_token: right::PreviewLoadToken,
    file_path: String,
    work: Work,
    apply: Apply,
) where
    T: Send + 'static,
    Work: FnOnce() -> T + Send + 'static,
    Apply: FnMut(&right::RightPane, T) + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = work();
        let _ = sender.send(result);
    });

    receive_preview_load(right, load_token, file_path, receiver, apply);
}

pub fn receive_preview_load<T, Apply>(
    right: Rc<right::RightPane>,
    load_token: right::PreviewLoadToken,
    file_path: String,
    receiver: mpsc::Receiver<T>,
    mut apply: Apply,
) where
    T: Send + 'static,
    Apply: FnMut(&right::RightPane, T) + 'static,
{
    gtk::glib::timeout_add_local(
        Duration::from_millis(PREVIEW_POLL_MS),
        move || match receiver.try_recv() {
            Ok(result) => {
                if right.is_current_load(load_token) {
                    apply(&right, result);
                } else {
                    log::debug!(
                        "preview load result ignored because token is stale file_path={file_path}"
                    );
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if right.is_current_load(load_token) {
                    right.show_unavailable(&file_path, "Preview loading did not return a result.");
                }
                gtk::glib::ControlFlow::Break
            }
        },
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentSignature {
    len: usize,
    hash: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskSignature {
    len: u64,
    modified: Option<SystemTime>,
}

pub fn content_signature(bytes: &[u8]) -> ContentSignature {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    ContentSignature {
        len: bytes.len(),
        hash: hasher.finish(),
    }
}

pub fn disk_signature(info: &FileNodeInfo) -> DiskSignature {
    DiskSignature {
        len: info.len_or_zero(),
        modified: info.modified,
    }
}
