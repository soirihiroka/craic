use super::*;
use craic_render_skia::{DragSelection, selected_editor_text};
use craic_ui_code::Motion;
use gtk::{gdk, gio};

pub(super) fn install(view: &Viewport) {
    let im = gtk::IMMulticontext::new();
    im.set_client_widget(Some(view));
    let keys = gtk::EventControllerKey::new();
    keys.set_im_context(Some(&im));
    im.connect_commit({
        let view = view.downgrade();
        move |_, text| {
            if let Some(view) = view.upgrade() {
                view.state().editor.borrow_mut().marked_text.clear();
                view.replace_text(text);
            }
        }
    });
    im.connect_preedit_changed({
        let view = view.downgrade();
        move |im| {
            if let Some(view) = view.upgrade() {
                let mut editor = view.state().editor.borrow_mut();
                if editor.editable {
                    editor.clear_completion();
                    editor.marked_text = im.preedit_string().0.to_string();
                    drop(editor);
                    view.queue_draw();
                }
            }
        }
    });
    im.connect_retrieve_surrounding({
        let view = view.downgrade();
        move |im| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            let editor = view.state().editor.borrow();
            im.set_surrounding_with_selection(
                editor.document.text(),
                editor.selection.focus as i32,
                editor.selection.anchor as i32,
            );
            true
        }
    });
    im.connect_delete_surrounding({
        let view = view.downgrade();
        move |_, offset, count| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            let mut editor = view.state().editor.borrow_mut();
            let text = editor.document.text();
            let focus = text[..editor.selection.focus].chars().count() as i64;
            let start = (focus + i64::from(offset)).max(0) as usize;
            let end = start.saturating_add(count.max(0) as usize);
            let byte = |character| {
                text.char_indices()
                    .nth(character)
                    .map_or(text.len(), |(byte, _)| byte)
            };
            let range = byte(start)..byte(end);
            let changed = editor.replace_text(Some(range), "");
            drop(editor);
            if changed {
                view.document_changed(true);
            }
            changed
        }
    });
    keys.connect_key_pressed({
        let view = view.downgrade();
        move |_, key, _, modifiers| {
            let Some(view) = view.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let handled = key_pressed(&view, key, modifiers);
            if handled {
                let editor = view.state().editor.borrow();
                let metrics = editor.metrics;
                let x = f64::from(metrics.gutter_width + metrics.text_inset)
                    + editor
                        .document
                        .visual_column_for_offset(editor.selection.focus)
                        * f64::from(metrics.char_width)
                    - editor.viewport.scroll_x();
                let y = f64::from(metrics.text_inset)
                    + editor
                        .document
                        .visual_line_for_offset(editor.selection.focus)
                        as f64
                        * f64::from(metrics.line_height)
                    - editor.viewport.scroll_y();
                im.set_cursor_location(&gdk::Rectangle::new(
                    x as i32,
                    y as i32,
                    1,
                    metrics.line_height.ceil() as i32,
                ));
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    view.add_controller(keys);

    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter({
        let view = view.downgrade();
        move |_| {
            if let Some(view) = view.upgrade() {
                view.queue_draw();
            }
        }
    });
    focus.connect_leave({
        let view = view.downgrade();
        move |_| {
            if let Some(view) = view.upgrade() {
                let mut editor = view.state().editor.borrow_mut();
                editor.marked_text.clear();
                editor.clear_completion();
                drop(editor);
                view.queue_draw();
            }
        }
    });
    view.add_controller(focus);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    scroll.connect_scroll({
        let view = view.downgrade();
        move |controller, dx, dy| {
            let Some(view) = view.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let scale = if controller.unit() == gdk::ScrollUnit::Wheel {
                f64::from(view.state().editor.borrow().metrics.line_height) * 3.0
            } else {
                1.0
            };
            view.scroll(dx * scale, dy * scale);
            glib::Propagation::Stop
        }
    });
    view.add_controller(scroll);

    let drag = Rc::new(Cell::new(None::<DragSelection<usize>>));
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    click.connect_pressed({
        let view = view.downgrade();
        let drag = Rc::clone(&drag);
        move |gesture, count, x, y| {
            let Some(view) = view.upgrade() else { return };
            view.grab_focus();
            let mut editor = view.state().editor.borrow_mut();
            editor.clear_completion();
            if x <= 24.0 && editor.toggle_fold(y) {
                drag.set(None);
                drop(editor);
                view.refresh();
                return;
            }
            let extend = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            drag.set(Some(editor.start_drag(x, y, count, extend)));
            drop(editor);
            view.refresh();
        }
    });
    let selection_drag = gtk::GestureDrag::new();
    selection_drag.set_button(gdk::BUTTON_PRIMARY);
    view.add_controller(click.clone());
    view.add_controller(selection_drag.clone());
    selection_drag.group_with(&click);
    selection_drag.connect_drag_update({
        let view = view.downgrade();
        move |gesture, dx, dy| {
            let (Some(view), Some(drag), Some((x, y))) =
                (view.upgrade(), drag.get(), gesture.start_point())
            else {
                return;
            };
            let (x, y) = (x + dx, y + dy);
            view.state().editor.borrow_mut().drag_to(drag, x, y);
            view.refresh();
        }
    });
    let context = gtk::GestureClick::new();
    context.set_button(gdk::BUTTON_SECONDARY);
    context.connect_pressed({
        let view = view.downgrade();
        move |_, _, x, y| {
            if let Some(view) = view.upgrade() {
                let offset = view.state().editor.borrow().hit_test(x, y);
                super::diagnostics::show_menu(&view, offset, x as i32, y as i32);
            }
        }
    });
    view.add_controller(context);
    view.set_has_tooltip(true);
    view.connect_query_tooltip(|view, x, y, keyboard, tooltip| {
        let editor = view.state().editor.borrow();
        let offset = if keyboard {
            editor.selection.focus
        } else {
            editor.hit_test(x.into(), y.into())
        };
        drop(editor);
        super::diagnostics::tooltip(view.state(), offset, tooltip)
    });
}

fn key_pressed(view: &Viewport, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    use gdk::Key;
    let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    let mut editor = view.state().editor.borrow_mut();
    let follow = match key {
        Key::Left | Key::Right => Some((
            if control {
                Motion::Word
            } else {
                Motion::Character
            },
            key == Key::Right,
        )),
        Key::Up | Key::Down => Some((Motion::Row, key == Key::Down)),
        Key::Home | Key::End => Some((
            if control {
                Motion::DocumentEdge
            } else {
                Motion::LineEdge
            },
            key == Key::End,
        )),
        _ => None,
    };
    if let Some((motion, forward)) = follow {
        if !matches!(motion, Motion::Row) || shift || !editor.step_completion(forward) {
            editor.move_selection(motion, forward, shift);
        }
        drop(editor);
        view.refresh();
        return true;
    }
    let changed = match key {
        Key::Page_Up | Key::Page_Down => {
            let delta = editor.viewport.height() * if key == Key::Page_Down { 1.0 } else { -1.0 };
            drop(editor);
            view.scroll(0.0, delta);
            return true;
        }
        Key::a | Key::A if control => {
            editor.selection = EditorSelection {
                anchor: 0,
                focus: editor.document.text().len(),
            };
            drop(editor);
            view.queue_draw();
            return true;
        }
        Key::c | Key::C | Key::x | Key::X | Key::v | Key::V if control => {
            drop(editor);
            view.clipboard_action(key.to_lower());
            return true;
        }
        Key::z | Key::Z if control => editor.undo(shift),
        Key::y | Key::Y if control => editor.undo(true),
        Key::slash | Key::question if control => editor.toggle_line_comment(),
        Key::plus | Key::equal | Key::KP_Add | Key::minus | Key::underscore | Key::KP_Subtract
            if control =>
        {
            let delta = if matches!(key, Key::minus | Key::underscore | Key::KP_Subtract) {
                -1.0
            } else {
                1.0
            };
            let size = f64::from(editor.metrics.font_size) + delta;
            drop(editor);
            config::save_editor_font_size(view.set_font_size(size));
            return true;
        }
        Key::space if control && editor.editable => {
            drop(editor);
            view.analyze(true);
            return true;
        }
        Key::Escape if !editor.completion_items.is_empty() => {
            editor.clear_completion();
            drop(editor);
            view.queue_draw();
            return true;
        }
        Key::BackSpace | Key::Delete if editor.editable => {
            editor.delete(key == Key::Delete, control)
        }
        Key::Return | Key::KP_Enter if editor.editable => {
            if editor.accept_completion() {
                true
            } else {
                let (start, _) = editor.selection.normalized();
                let (line, _) = editor.document.line_column_for_offset(start);
                let line_start = editor.document.lines()[line].start;
                let leading: String = editor.document.text()[line_start..start]
                    .chars()
                    .take_while(|character| matches!(character, ' ' | '\t'))
                    .collect();
                editor.replace_text(None, &format!("\n{leading}"))
            }
        }
        Key::Tab if editor.editable && !shift => {
            editor.accept_completion() || editor.replace_text(None, "    ")
        }
        _ => return false,
    };
    drop(editor);
    if changed {
        view.document_changed(true);
    }
    true
}

impl Viewport {
    pub(super) fn replace_text(&self, replacement: &str) {
        let changed = self
            .state()
            .editor
            .borrow_mut()
            .replace_text(None, replacement);
        if changed {
            self.document_changed(true);
        }
    }

    pub(super) fn clipboard_action(&self, key: gdk::Key) {
        let clipboard = self.clipboard();
        if key == gdk::Key::v {
            let view = self.downgrade();
            let revision = self.state().revision.get();
            clipboard.read_text_async(gio::Cancellable::NONE, move |result| {
                if let (Ok(Some(text)), Some(view)) = (result, view.upgrade())
                    && view.state().revision.get() == revision
                {
                    view.replace_text(&text);
                }
            });
        } else {
            let editor = self.state().editor.borrow();
            let text = selected_editor_text(&editor.document, editor.selection);
            drop(editor);
            if !text.is_empty() {
                clipboard.set_text(&text);
                if key == gdk::Key::x {
                    self.replace_text("");
                }
            }
        }
    }
}
