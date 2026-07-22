use super::image_viewer;
use super::{DiskSignature, PreviewMatchRequest, PreviewRequest, disk_signature};
use crate::system::materialize::MaterializedFile;
use crate::ui::widgets;
use gtk::{gio, prelude::*};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

const MAX_MEDIA_PREVIEW_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Audio,
    Video,
}

impl MediaKind {
    fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MediaPreviewState {
    kind: MediaKind,
    file_path: String,
    disk_signature: DiskSignature,
}

pub struct MediaPreview {
    pub root: gtk::Box,
    stack: gtk::Stack,
    image_viewer: Rc<image_viewer::ImageViewer>,
    video: gtk::Video,
    audio_controls: gtk::MediaControls,
    message: gtk::Label,
    audio_stream: RefCell<Option<gtk::MediaFile>>,
    video_stream: RefCell<Option<gtk::MediaFile>>,
    materialized: RefCell<Option<MaterializedFile>>,
    state: RefCell<Option<MediaPreviewState>>,
}

struct MediaPreviewLoad {
    kind: MediaKind,
    file_path: String,
    full_path: PathBuf,
    materialized: Option<MaterializedFile>,
    disk_signature: DiskSignature,
}

impl MediaPreviewLoad {
    fn gio_file(&self) -> gio::File {
        gio::File::for_path(&self.full_path)
    }
}

impl MediaPreview {
    pub fn new() -> Rc<Self> {
        let image_viewer = image_viewer::ImageViewer::new();

        let video = gtk::Video::builder()
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build();
        video.set_autoplay(false);
        video.set_loop(false);

        let audio_icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        audio_icon.set_pixel_size(96);
        audio_icon.add_css_class("dim-label");
        audio_icon.set_halign(gtk::Align::Center);
        audio_icon.set_valign(gtk::Align::Center);

        let audio_controls = gtk::MediaControls::new(gtk::MediaStream::NONE);
        audio_controls.set_hexpand(true);
        audio_controls.set_vexpand(true);
        audio_controls.set_halign(gtk::Align::Fill);
        audio_controls.set_valign(gtk::Align::Center);
        audio_controls.set_size_request(640, -1);

        let audio_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(32)
            .margin_bottom(32)
            .margin_start(32)
            .margin_end(32)
            .build();
        audio_box.append(&audio_icon);
        audio_box.append(&audio_controls);

        let audio_clamp = adw::Clamp::builder()
            .orientation(gtk::Orientation::Horizontal)
            .maximum_size(1600)
            .tightening_threshold(1080)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .child(&audio_box)
            .build();

        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.add_named(&image_viewer.root, Some("image"));
        stack.add_named(&video, Some("video"));
        stack.add_named(&audio_clamp, Some("audio"));
        stack.set_visible_child_name("image");

        let message = widgets::muted("");
        message.set_halign(gtk::Align::Center);
        message.set_margin_top(8);
        message.set_margin_bottom(12);
        message.set_margin_start(18);
        message.set_margin_end(18);
        message.set_visible(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&stack);
        root.append(&message);

        Rc::new(Self {
            root,
            stack,
            image_viewer,
            video,
            audio_controls,
            message,
            audio_stream: RefCell::new(None),
            video_stream: RefCell::new(None),
            materialized: RefCell::new(None),
            state: RefCell::new(None),
        })
    }

    pub fn clear(&self) {
        self.state.borrow_mut().take();
        if let Some(stream) = self.audio_stream.borrow().as_ref() {
            stream.pause();
            stream.clear();
        }
        if let Some(stream) = self.video_stream.borrow().as_ref() {
            stream.pause();
            stream.clear();
        }

        self.audio_controls.set_media_stream(gtk::MediaStream::NONE);
        self.video.set_media_stream(gtk::MediaStream::NONE);
        self.image_viewer.clear();
        self.message.set_visible(false);
        self.audio_stream.borrow_mut().take();
        self.video_stream.borrow_mut().take();
        self.materialized.borrow_mut().take();
    }

    fn set_media(&self, load: MediaPreviewLoad) {
        if self.preview_matches(load.kind, &load.file_path, load.disk_signature) {
            return;
        }

        self.clear();
        let file = load.gio_file();
        match load.kind {
            MediaKind::Image => self.set_image_file(&load.file_path, &file),
            MediaKind::Audio => self.set_audio_file(&file),
            MediaKind::Video => self.set_video_file(&file),
        }
        self.materialized.replace(load.materialized);
        self.set_state(load.kind, &load.file_path, load.disk_signature);
    }

    fn set_image_file(&self, file_path: &str, file: &gio::File) {
        self.image_viewer.set_file(file_path, file);
        self.stack.set_visible_child_name("image");
    }

    fn set_audio_file(&self, file: &gio::File) {
        let stream = gtk::MediaFile::for_file(file);
        attach_media_error(&stream, &self.message, "audio");
        self.audio_controls.set_media_stream(Some(&stream));
        self.audio_stream.replace(Some(stream));
        self.stack.set_visible_child_name("audio");
    }

    fn set_video_file(&self, file: &gio::File) {
        let stream = gtk::MediaFile::for_file(file);
        attach_media_error(&stream, &self.message, "video");
        self.video.set_media_stream(Some(&stream));
        self.video_stream.replace(Some(stream));
        self.stack.set_visible_child_name("video");
    }

    fn preview_matches(
        &self,
        kind: MediaKind,
        file_path: &str,
        disk_signature: DiskSignature,
    ) -> bool {
        self.state.borrow().as_ref().is_some_and(|state| {
            state.kind == kind
                && state.file_path == file_path
                && state.disk_signature == disk_signature
        })
    }

    fn set_state(&self, kind: MediaKind, file_path: &str, disk_signature: DiskSignature) {
        self.state.replace(Some(MediaPreviewState {
            kind,
            file_path: file_path.to_string(),
            disk_signature,
        }));
    }
}

pub fn show_image(request: PreviewRequest<'_>) {
    show_media(request, MediaKind::Image);
}

pub fn show_audio(request: PreviewRequest<'_>) {
    show_media(request, MediaKind::Audio);
}

pub fn show_video(request: PreviewRequest<'_>) {
    show_media(request, MediaKind::Video);
}

pub fn show_image_match(request: PreviewMatchRequest<'_>) {
    show_image(preview_request_from_match(request));
}

pub fn show_audio_match(request: PreviewMatchRequest<'_>) {
    show_audio(preview_request_from_match(request));
}

pub fn show_video_match(request: PreviewMatchRequest<'_>) {
    show_video(preview_request_from_match(request));
}

fn show_media(request: PreviewRequest<'_>, kind: MediaKind) {
    request
        .right
        .show_provider_loading(request.load_token, request.file_path, kind.label());

    let file_path = request.file_path.to_string();
    let local_path = request.local_path.map(|path| path.to_path_buf());
    let files = request.files.clone();
    let source = request.info.clone();
    let is_file = request.info.kind.is_file();
    let disk_signature = disk_signature(request.info);
    let apply_file_path = file_path.clone();

    let (sender, receiver) = mpsc::channel();
    if !is_file {
        let _ = sender.send(Err("Select a file to preview.".to_string()));
    } else if let Some(path) = local_path {
        let _ = sender.send(Ok(MediaPreviewLoad {
            kind,
            file_path,
            full_path: path,
            materialized: None,
            disk_signature,
        }));
    } else {
        crate::system::materialize::materialize_for_view(
            files,
            source,
            Some(MAX_MEDIA_PREVIEW_BYTES),
            move |result| {
                let result = result.map(|materialized| MediaPreviewLoad {
                    kind,
                    file_path,
                    full_path: materialized.path().to_path_buf(),
                    materialized: Some(materialized),
                    disk_signature,
                });
                let _ = sender.send(result);
            },
        );
    }

    super::receive_preview_load(
        request.right,
        request.load_token,
        apply_file_path.clone(),
        receiver,
        move |right, result| match result {
            Ok(load) => {
                let file_path = load.file_path.clone();
                right.file_media_preview.set_media(load);
                right.show_media_preview(&file_path, "");
            }
            Err(message) => right.show_unavailable(&apply_file_path, &message),
        },
    );
}

fn preview_request_from_match(request: PreviewMatchRequest<'_>) -> PreviewRequest<'_> {
    request.into_preview_request()
}

fn attach_media_error(stream: &gtk::MediaFile, message: &gtk::Label, kind: &'static str) {
    let message = message.clone();
    stream.connect_error_notify(move |stream| {
        let Some(error) = stream.error() else {
            message.set_visible(false);
            return;
        };

        message.set_label(&format!("Unable to play {kind}: {error}"));
        message.set_visible(true);
    });
}
