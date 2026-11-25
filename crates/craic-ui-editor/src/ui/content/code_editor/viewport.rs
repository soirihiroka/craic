use super::EditorState;
use craic_ui_code::{EditorFrame, EditorScene, paint_editor};
use gpui::PlatformHeadlessRenderer;
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, subclass::prelude::*};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::{OnceLock, mpsc};
use tokio::sync::oneshot;

type Pixels = (i32, i32, Vec<u8>);

struct RenderRequest {
    frame: EditorFrame,
    reply: oneshot::Sender<Result<Pixels, String>>,
}

static RENDERER: OnceLock<Result<mpsc::Sender<RenderRequest>, String>> = OnceLock::new();

fn render_queue() -> Result<&'static mpsc::Sender<RenderRequest>, &'static str> {
    RENDERER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<RenderRequest>();
            std::thread::Builder::new()
                .name("craic-gpui-editor".into())
                .spawn(move || {
                    let mut renderer = match WgpuHeadlessRenderer::new() {
                        Ok(renderer) => renderer,
                        Err(error) => {
                            log::warn!("GPUI code editor GPU initialization failed: {error:#}");
                            return;
                        }
                    };
                    let text = CosmicTextSystem::new(craic_ui_code::EDITOR_FONT_FAMILY);
                    if let Err(error) = craic_ui_code::load_editor_font(&text) {
                        log::warn!("GPUI code editor font initialization failed: {error}");
                        return;
                    }
                    let atlas = renderer.sprite_atlas();
                    let mut scene = EditorScene::default();
                    log::debug!("initialized the GPUI code editor renderer for GTK");
                    while let Ok(request) = receiver.recv() {
                        if request.reply.is_closed() {
                            continue;
                        }
                        let result = request
                            .frame
                            .build_scene(&mut scene, &text, atlas.as_ref())
                            .map_err(|error| error.to_string())
                            .and_then(|_| {
                                renderer
                                    .render_scene_to_image(
                                        &scene.scene,
                                        request.frame.device_size(),
                                    )
                                    .map_err(|error| format!("{error:#}"))
                            })
                            .map(|image| {
                                (
                                    image.width() as i32,
                                    image.height() as i32,
                                    image.into_raw(),
                                )
                            });
                        let _ = request.reply.send(result);
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(sender)
        })
        .as_ref()
        .map_err(String::as_str)
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Viewport {
        pub(super) state: OnceCell<Rc<EditorState>>,
        pub(super) cached: RefCell<Option<(EditorFrame, gdk::Texture)>>,
        pub(super) pending: RefCell<Option<glib::JoinHandle<()>>>,
        failed: Cell<bool>,
        pub(super) theme_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Viewport {
        const NAME: &'static str = "CraicGpuiCodeViewport";
        type Type = super::Viewport;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_accessible_role(gtk::AccessibleRole::TextBox);
        }
    }

    impl ObjectImpl for Viewport {
        fn dispose(&self) {
            let view = self.obj();
            view.clear_frame();
            if let Some(handler) = self.theme_handler.borrow_mut().take() {
                adw::StyleManager::default().disconnect(handler);
            }
            if let Some(child) = view.first_child() {
                child.unparent();
            }
            if let Some(state) = self.state.get()
                && let Some(task) = state.analysis_task.borrow_mut().take()
            {
                task.abort();
            }
        }
    }

    impl WidgetImpl for Viewport {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let view = self.obj();
            view.state()
                .editor
                .borrow_mut()
                .resize(width.into(), height.into());
            // Adjustment callbacks can scroll the Markdown preview and resize us.
            let view = view.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(view) = view.upgrade() {
                    view.refresh();
                }
            });
        }

        fn unmap(&self) {
            self.obj().clear_frame();
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let view = self.obj();
            let width = view.width() as f32;
            let height = view.height() as f32;
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            if let Some((_, texture)) = self.cached.borrow().as_ref() {
                snapshot.append_texture(texture, &bounds);
            } else {
                snapshot.append_color(&gdk::RGBA::new(0.118, 0.118, 0.118, 1.0), &bounds);
            }
            if self.failed.get() {
                if self.cached.borrow().is_none() {
                    let layout =
                        view.create_pango_layout(Some("Code editor rendering is unavailable."));
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(12.0, 12.0));
                    snapshot.append_layout(&layout, &gdk::RGBA::WHITE);
                    snapshot.restore();
                }
                return;
            }
            if self.pending.borrow().is_some() {
                return;
            }

            let frame = view.frame(width, height);
            if self
                .cached
                .borrow()
                .as_ref()
                .is_some_and(|(previous, _)| *previous == frame)
            {
                return;
            }
            let (reply, receiver) = oneshot::channel();
            let sender = match render_queue() {
                Ok(sender) => sender,
                Err(error) => {
                    self.failed.set(true);
                    log::warn!("failed to start the GPUI code editor renderer: {error}");
                    return;
                }
            };
            if sender
                .send(RenderRequest {
                    frame: frame.clone(),
                    reply,
                })
                .is_err()
            {
                self.failed.set(true);
                view.queue_draw();
                return;
            }
            let view = view.downgrade();
            let task = glib::spawn_future_local(async move {
                let result = receiver.await;
                let Some(view) = view.upgrade() else { return };
                let imp = view.imp();
                imp.pending.borrow_mut().take();
                match result {
                    Ok(Ok((width, height, bytes))) => {
                        let texture = gdk::MemoryTexture::new(
                            width,
                            height,
                            gdk::MemoryFormat::R8g8b8a8,
                            &glib::Bytes::from_owned(bytes),
                            width as usize * 4,
                        );
                        imp.cached.replace(Some((frame, texture.upcast())));
                    }
                    Ok(Err(error)) => {
                        imp.failed.set(true);
                        log::warn!("failed to draw the GTK code editor with GPUI: {error}");
                    }
                    Err(_) => imp.failed.set(true),
                }
                view.queue_draw();
            });
            self.pending.replace(Some(task));
        }
    }
}

glib::wrapper! {
    pub struct Viewport(ObjectSubclass<imp::Viewport>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Viewport {
    pub(super) fn new(state: Rc<EditorState>) -> Self {
        let view: Self = glib::Object::builder()
            .property("hexpand", true)
            .property("vexpand", true)
            .property("focusable", true)
            .build();
        assert!(view.imp().state.set(state).is_ok());
        view.set_cursor_from_name(Some("text"));
        let weak = view.downgrade();
        let handler = adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(view) = weak.upgrade() {
                view.queue_draw();
            }
        });
        view.imp().theme_handler.replace(Some(handler));
        view.update_property(&[
            gtk::accessible::Property::Label("Code editor"),
            gtk::accessible::Property::MultiLine(true),
            gtk::accessible::Property::ReadOnly(true),
        ]);
        view
    }

    pub(super) fn state(&self) -> &EditorState {
        self.imp().state.get().expect("code viewport state")
    }

    pub(super) fn clear_frame(&self) {
        let imp = self.imp();
        if let Some(task) = imp.pending.borrow_mut().take() {
            task.abort();
        }
        imp.cached.borrow_mut().take();
    }

    fn frame(&self, width: f32, height: f32) -> EditorFrame {
        let state = self.state();
        let editor = state.editor.borrow();
        let metrics = editor.metrics;
        let mut frame = EditorFrame::new(width, height, self.scale_factor() as f32);
        paint_editor(
            &mut frame,
            &editor,
            editor.editable && self.has_focus(),
            adw::StyleManager::default().is_dark(),
        );
        let marks = state.diff_marks.borrow();
        for row in
            editor
                .document
                .visible_line_range(editor.viewport.scroll_y(), height as f64, metrics)
        {
            let line = &editor.document.visual_lines()[row];
            if line.wrap_index != 0 {
                continue;
            }
            if let Some(&color) = marks.get(&line.source_line) {
                let y = metrics.text_inset + row as f32 * metrics.line_height
                    - editor.viewport.scroll_y() as f32;
                frame.quad(
                    metrics.gutter_width - 3.0,
                    y,
                    3.0,
                    metrics.line_height,
                    0.0,
                    color,
                );
            }
        }
        frame
    }
}
