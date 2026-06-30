mod folder;
mod font;
pub(in crate::ui::pages::code) mod markdown;
pub(in crate::ui::pages::code) mod media;
pub(in crate::ui::pages::code) mod notebook;
mod notebook_readonly;
mod pdf;
pub(in crate::ui::pages::code) mod sqlite;
pub(in crate::ui::pages::code) mod svg;
mod text;

use super::{PageContext, right};
use crate::system::WorkspacePath;
use crate::system::capabilities::files::{FileAccess, FileKind, FileMetadata};
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

pub(in crate::ui::pages::code) type PreviewFn = for<'a> fn(PreviewRequest<'a>);
pub(in crate::ui::pages::code) type MatchPreviewFn = for<'a> fn(PreviewMatchRequest<'a>);

pub(in crate::ui::pages::code) struct Provider {
    pub(in crate::ui::pages::code) show: PreviewFn,
    pub(in crate::ui::pages::code) show_match: MatchPreviewFn,
}

pub(in crate::ui::pages::code) struct PreviewRequest<'a> {
    pub(in crate::ui::pages::code) ctx: PageContext,
    pub(in crate::ui::pages::code) right: Rc<right::RightPane>,
    pub(in crate::ui::pages::code) load_token: right::PreviewLoadToken,
    pub(in crate::ui::pages::code) files: Arc<dyn FileAccess>,
    pub(in crate::ui::pages::code) file_path: &'a str,
    pub(in crate::ui::pages::code) workspace_path: &'a WorkspacePath,
    pub(in crate::ui::pages::code) local_path: Option<&'a Path>,
    pub(in crate::ui::pages::code) metadata: &'a FileMetadata,
    pub(in crate::ui::pages::code) prefetched_bytes: Option<&'a [u8]>,
}

pub(in crate::ui::pages::code) struct PreviewMatchRequest<'a> {
    pub(in crate::ui::pages::code) ctx: PageContext,
    pub(in crate::ui::pages::code) right: Rc<right::RightPane>,
    pub(in crate::ui::pages::code) load_token: right::PreviewLoadToken,
    pub(in crate::ui::pages::code) files: Arc<dyn FileAccess>,
    pub(in crate::ui::pages::code) file_path: &'a str,
    pub(in crate::ui::pages::code) workspace_path: &'a WorkspacePath,
    pub(in crate::ui::pages::code) local_path: Option<&'a Path>,
    pub(in crate::ui::pages::code) metadata: &'a FileMetadata,
    pub(in crate::ui::pages::code) prefetched_bytes: Option<&'a [u8]>,
    pub(in crate::ui::pages::code) start: usize,
    pub(in crate::ui::pages::code) end: usize,
}

impl<'a> PreviewMatchRequest<'a> {
    pub(in crate::ui::pages::code) fn into_preview_request(self) -> PreviewRequest<'a> {
        PreviewRequest {
            ctx: self.ctx,
            right: self.right,
            load_token: self.load_token,
            files: self.files,
            file_path: self.file_path,
            workspace_path: self.workspace_path,
            local_path: self.local_path,
            metadata: self.metadata,
            prefetched_bytes: self.prefetched_bytes,
        }
    }
}

pub(in crate::ui::pages::code) fn for_file(
    file_path: &str,
    metadata: &FileMetadata,
    prefetched_bytes: Option<&[u8]>,
) -> Provider {
    let is_file = metadata.kind == FileKind::File;
    let is_dir = metadata.kind == FileKind::Directory;
    let path_preview_kind = file_type::preview_kind_for_path(file_path, is_dir);
    let preview_kind = if is_file
        && path_preview_kind != file_type::PreviewKind::Sqlite
        && prefetched_bytes.is_some_and(sqlite::has_sqlite_magic_bytes)
    {
        log::debug!("sqlite preview selected by magic file_path={file_path}");
        file_type::PreviewKind::Sqlite
    } else {
        path_preview_kind
    };

    if preview_kind == file_type::PreviewKind::Sqlite {
        log::debug!("sqlite preview selected file_path={file_path}");
    }

    match preview_kind {
        file_type::PreviewKind::Folder => Provider {
            show: folder::show,
            show_match: folder::show_match,
        },
        file_type::PreviewKind::Notebook => Provider {
            show: notebook::show,
            show_match: notebook::show_match,
        },
        file_type::PreviewKind::Svg => Provider {
            show: svg::show,
            show_match: svg::show_match,
        },
        file_type::PreviewKind::Markdown => Provider {
            show: markdown::show,
            show_match: markdown::show_match,
        },
        file_type::PreviewKind::Image => Provider {
            show: media::show_image,
            show_match: media::show_image_match,
        },
        file_type::PreviewKind::Audio => Provider {
            show: media::show_audio,
            show_match: media::show_audio_match,
        },
        file_type::PreviewKind::Video => Provider {
            show: media::show_video,
            show_match: media::show_video_match,
        },
        file_type::PreviewKind::Font => Provider {
            show: font::show,
            show_match: font::show_match,
        },
        file_type::PreviewKind::Pdf => Provider {
            show: pdf::show,
            show_match: pdf::show_match,
        },
        file_type::PreviewKind::Sqlite => Provider {
            show: sqlite::show,
            show_match: sqlite::show_match,
        },
        file_type::PreviewKind::Text => Provider {
            show: text::show,
            show_match: text::show_match,
        },
    }
}

pub(in crate::ui::pages::code) fn spawn_preview_load<T, Work, Apply>(
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

fn receive_preview_load<T, Apply>(
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
pub(in crate::ui::pages::code) struct ContentSignature {
    len: usize,
    hash: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui::pages::code) struct DiskSignature {
    len: u64,
    modified: Option<SystemTime>,
}

pub(in crate::ui::pages::code) fn content_signature(bytes: &[u8]) -> ContentSignature {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    ContentSignature {
        len: bytes.len(),
        hash: hasher.finish(),
    }
}

pub(in crate::ui::pages::code) fn disk_signature(metadata: &FileMetadata) -> DiskSignature {
    DiskSignature {
        len: metadata.len,
        modified: metadata.modified,
    }
}
