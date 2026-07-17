use gtk::gdk;
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

type FileHoverCallback =
    Rc<dyn Fn(Option<Vec<PathBuf>>, f64, gdk::DragAction, gdk::ModifierType) -> gdk::DragAction>;
type FileDropCallback = Rc<dyn Fn(Vec<PathBuf>, f64, gdk::DragAction, gdk::ModifierType) -> bool>;
type AsyncHoverCallback = Rc<dyn Fn(f64, gdk::DragAction, gdk::ModifierType) -> gdk::DragAction>;
type AsyncDropCallback = Rc<dyn Fn(gdk::Drop, f64, gdk::DragAction, gdk::ModifierType) -> bool>;
type VoidCallback = Rc<dyn Fn()>;
type DragPrepareCallback = Rc<dyn Fn() -> Option<gdk::ContentProvider>>;

#[derive(Clone)]
pub struct FileDropTarget {
    mime_types: &'static [&'static str],
    actions: gdk::DragAction,
    on_file_hover: Option<FileHoverCallback>,
    on_file_drop: Option<FileDropCallback>,
    on_async_hover: Option<AsyncHoverCallback>,
    on_async_drop: Option<AsyncDropCallback>,
    on_leave: Option<VoidCallback>,
}

impl FileDropTarget {
    pub fn builder(mime_types: &'static [&'static str]) -> FileDropTargetBuilder {
        FileDropTargetBuilder {
            mime_types,
            actions: gdk::DragAction::COPY | gdk::DragAction::MOVE,
            on_file_hover: None,
            on_file_drop: None,
            on_async_hover: None,
            on_async_drop: None,
            on_leave: None,
        }
    }

    pub fn install<W>(&self, widget: &W)
    where
        W: IsA<gtk::Widget>,
    {
        self.install_file_list_target(widget);
        self.install_async_target(widget);
    }

    fn install_file_list_target<W>(&self, widget: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), self.actions);
        target.connect_enter({
            let on_file_hover = self.on_file_hover.clone();

            move |target, _, y| {
                on_file_hover
                    .as_ref()
                    .map(|callback| {
                        callback(
                            Some(Vec::new()),
                            y,
                            available_drop_actions(
                                target.current_drop().as_ref(),
                                target.actions(),
                            ),
                            target.current_event_state(),
                        )
                    })
                    .unwrap_or_else(gdk::DragAction::empty)
            }
        });
        target.connect_motion({
            let on_file_hover = self.on_file_hover.clone();

            move |target, _, y| {
                on_file_hover
                    .as_ref()
                    .map(|callback| {
                        callback(
                            Some(Vec::new()),
                            y,
                            available_drop_actions(
                                target.current_drop().as_ref(),
                                target.actions(),
                            ),
                            target.current_event_state(),
                        )
                    })
                    .unwrap_or_else(gdk::DragAction::empty)
            }
        });
        target.connect_drop({
            let on_file_drop = self.on_file_drop.clone();

            move |target, value, _, y| {
                let Some(sources) = file_list_value_paths(value) else {
                    return false;
                };
                on_file_drop.as_ref().is_some_and(|callback| {
                    callback(
                        sources,
                        y,
                        available_drop_actions(target.current_drop().as_ref(), target.actions()),
                        target.current_event_state(),
                    )
                })
            }
        });
        target.connect_leave({
            let on_leave = self.on_leave.clone();

            move |_| {
                if let Some(callback) = &on_leave {
                    callback();
                }
            }
        });
        widget.add_controller(target);
    }

    fn install_async_target<W>(&self, widget: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let target = gtk::DropTargetAsync::new(
            Some(gdk::ContentFormats::new(self.mime_types)),
            self.actions,
        );
        target.connect_drag_enter({
            let on_async_hover = self.on_async_hover.clone();

            move |target, drop, _, y| {
                on_async_hover
                    .as_ref()
                    .map(|callback| {
                        callback(
                            y,
                            available_drop_actions(Some(drop), target.actions()),
                            target.current_event_state(),
                        )
                    })
                    .unwrap_or_else(gdk::DragAction::empty)
            }
        });
        target.connect_drag_motion({
            let on_async_hover = self.on_async_hover.clone();

            move |target, drop, _, y| {
                on_async_hover
                    .as_ref()
                    .map(|callback| {
                        callback(
                            y,
                            available_drop_actions(Some(drop), target.actions()),
                            target.current_event_state(),
                        )
                    })
                    .unwrap_or_else(gdk::DragAction::empty)
            }
        });
        target.connect_drop({
            let on_async_drop = self.on_async_drop.clone();

            move |target, drop, _, y| {
                on_async_drop.as_ref().is_some_and(|callback| {
                    callback(
                        drop.clone(),
                        y,
                        available_drop_actions(Some(drop), target.actions()),
                        target.current_event_state(),
                    )
                })
            }
        });
        target.connect_drag_leave({
            let on_leave = self.on_leave.clone();

            move |_, _| {
                if let Some(callback) = &on_leave {
                    callback();
                }
            }
        });
        widget.add_controller(target);
    }
}

pub struct FileDropTargetBuilder {
    mime_types: &'static [&'static str],
    actions: gdk::DragAction,
    on_file_hover: Option<FileHoverCallback>,
    on_file_drop: Option<FileDropCallback>,
    on_async_hover: Option<AsyncHoverCallback>,
    on_async_drop: Option<AsyncDropCallback>,
    on_leave: Option<VoidCallback>,
}

impl FileDropTargetBuilder {
    pub fn on_file_hover<F>(mut self, callback: F) -> Self
    where
        F: Fn(Option<Vec<PathBuf>>, f64, gdk::DragAction, gdk::ModifierType) -> gdk::DragAction
            + 'static,
    {
        self.on_file_hover = Some(Rc::new(callback));
        self
    }

    pub fn on_file_drop<F>(mut self, callback: F) -> Self
    where
        F: Fn(Vec<PathBuf>, f64, gdk::DragAction, gdk::ModifierType) -> bool + 'static,
    {
        self.on_file_drop = Some(Rc::new(callback));
        self
    }

    pub fn on_async_hover<F>(mut self, callback: F) -> Self
    where
        F: Fn(f64, gdk::DragAction, gdk::ModifierType) -> gdk::DragAction + 'static,
    {
        self.on_async_hover = Some(Rc::new(callback));
        self
    }

    pub fn on_async_drop<F>(mut self, callback: F) -> Self
    where
        F: Fn(gdk::Drop, f64, gdk::DragAction, gdk::ModifierType) -> bool + 'static,
    {
        self.on_async_drop = Some(Rc::new(callback));
        self
    }

    pub fn on_leave<F>(mut self, callback: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.on_leave = Some(Rc::new(callback));
        self
    }

    pub fn build(self) -> FileDropTarget {
        FileDropTarget {
            mime_types: self.mime_types,
            actions: self.actions,
            on_file_hover: self.on_file_hover,
            on_file_drop: self.on_file_drop,
            on_async_hover: self.on_async_hover,
            on_async_drop: self.on_async_drop,
            on_leave: self.on_leave,
        }
    }
}

#[derive(Clone)]
pub struct DragSource {
    actions: gdk::DragAction,
    prepare: Option<DragPrepareCallback>,
    on_begin: Option<VoidCallback>,
    on_cancel: Option<Rc<dyn Fn() -> bool>>,
    on_end: Option<VoidCallback>,
}

impl DragSource {
    pub fn builder() -> DragSourceBuilder {
        DragSourceBuilder {
            actions: gdk::DragAction::COPY | gdk::DragAction::MOVE,
            prepare: None,
            on_begin: None,
            on_cancel: None,
            on_end: None,
        }
    }

    pub fn install<W>(&self, widget: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let source = gtk::DragSource::builder()
            .actions(self.actions)
            .button(1)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        source.connect_prepare({
            let prepare = self.prepare.clone();

            move |_, _, _| prepare.as_ref().and_then(|callback| callback())
        });
        source.connect_drag_begin({
            let on_begin = self.on_begin.clone();

            move |_, _| {
                if let Some(callback) = &on_begin {
                    callback();
                }
            }
        });
        source.connect_drag_cancel({
            let on_cancel = self.on_cancel.clone();

            move |_, _, _| on_cancel.as_ref().is_some_and(|callback| callback())
        });
        source.connect_drag_end({
            let on_end = self.on_end.clone();

            move |_, _, _| {
                if let Some(callback) = &on_end {
                    callback();
                }
            }
        });
        widget.add_controller(source);
    }
}

pub struct DragSourceBuilder {
    actions: gdk::DragAction,
    prepare: Option<DragPrepareCallback>,
    on_begin: Option<VoidCallback>,
    on_cancel: Option<Rc<dyn Fn() -> bool>>,
    on_end: Option<VoidCallback>,
}

impl DragSourceBuilder {
    pub fn prepare<F>(mut self, callback: F) -> Self
    where
        F: Fn() -> Option<gdk::ContentProvider> + 'static,
    {
        self.prepare = Some(Rc::new(callback));
        self
    }

    pub fn on_begin<F>(mut self, callback: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.on_begin = Some(Rc::new(callback));
        self
    }

    pub fn on_cancel<F>(mut self, callback: F) -> Self
    where
        F: Fn() -> bool + 'static,
    {
        self.on_cancel = Some(Rc::new(callback));
        self
    }

    pub fn on_end<F>(mut self, callback: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.on_end = Some(Rc::new(callback));
        self
    }

    pub fn build(self) -> DragSource {
        DragSource {
            actions: self.actions,
            prepare: self.prepare,
            on_begin: self.on_begin,
            on_cancel: self.on_cancel,
            on_end: self.on_end,
        }
    }
}

fn file_list_value_paths(value: &gtk::glib::Value) -> Option<Vec<PathBuf>> {
    let file_list = value.get::<gdk::FileList>().ok()?;
    let paths = file_list
        .files()
        .into_iter()
        .filter_map(|file| file.path())
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

fn available_drop_actions(
    drop: Option<&gdk::Drop>,
    target_actions: gdk::DragAction,
) -> gdk::DragAction {
    let Some(drop) = drop else {
        return target_actions;
    };
    let actions = drop.actions() & target_actions;
    if actions.is_empty() {
        target_actions
    } else {
        actions
    }
}
