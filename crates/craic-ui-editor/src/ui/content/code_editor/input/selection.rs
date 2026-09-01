fn install_editor_middle_autoscroll(area: &gtk::GLArea, state: &Rc<EditorState>) {
    canvas_scroll::install_middle_autoscroll(
        area,
        &state.middle_autoscroll,
        canvas_scroll::AutoscrollAxes::Vertical,
        "code_editor",
        {
            let area = area.clone();
            let state = state.clone();
            move || {
                let viewport_height = area.allocated_height().max(1) as f64;
                render::max_scroll_y(&state, viewport_height) > f64::EPSILON
            }
        },
        {
            let area = area.clone();
            let state = state.clone();
            move |autoscroll_state| {
                let viewport_height = area.allocated_height().max(1) as f64;
                let max_scroll = render::max_scroll_y(&state, viewport_height);
                if max_scroll <= f64::EPSILON {
                    return;
                }

                let delta = canvas_scroll::middle_autoscroll_delta(
                    autoscroll_state.pointer.y - autoscroll_state.origin.y,
                );
                if delta.abs() <= f64::EPSILON {
                    return;
                }

                canvas_overshoot::pull_for_delta(
                    &area,
                    &state.overshoot,
                    state.scroll_y.get(),
                    max_scroll,
                    delta,
                    canvas_overshoot::Edge::Top,
                    canvas_overshoot::Edge::Bottom,
                );
                render::set_scroll_y(&area, &state, state.scroll_y.get() + delta);
            }
        },
        {
            let area = area.clone();
            let state = state.clone();
            move || {
                clear_editor_autoscroll_hover(&area, &state);
                dismiss_completion(&state);
            }
        },
        {
            let area = area.clone();
            let state = state.clone();
            move || clear_editor_autoscroll_hover(&area, &state)
        },
        {
            let area = area.clone();
            move |cursor| area.set_cursor_from_name(cursor)
        },
        {
            let area = area.clone();
            move || area.queue_render()
        },
    );
}

fn clear_editor_autoscroll_hover(area: &gtk::GLArea, state: &Rc<EditorState>) {
    canvas_scrollbar::set_hover(
        area,
        &state.scrollbar_hover,
        &state.scrollbar_active,
        &state.scrollbar_hover_progress,
        &state.scrollbar_animating,
        false,
    );
    set_fold_hover(area, state, None);
    set_fold_pressed(area, state, None);
}

fn install_im_context(area: &gtk::GLArea, state: &Rc<EditorState>, keys: &gtk::EventControllerKey) {
    let im_context = gtk::IMMulticontext::new();
    im_context.set_client_widget(Some(area));
    im_context.set_use_preedit(true);

    im_context.connect_commit({
        let area = area.clone();
        let state = state.clone();
        move |_, text| {
            if !state.editable.get() || text.is_empty() {
                return;
            }
            state.preedit.borrow_mut().clear();
            insert_text(&area, &state, text);
        }
    });
    im_context.connect_preedit_changed({
        let area = area.clone();
        let state = state.clone();
        move |context| {
            let (preedit, _, _) = context.preedit_string();
            state.preedit.replace(preedit.to_string());
            area.queue_render();
        }
    });
    im_context.connect_preedit_end({
        let area = area.clone();
        let state = state.clone();
        move |_| {
            state.preedit.borrow_mut().clear();
            area.queue_render();
        }
    });
    area.connect_has_focus_notify({
        let state = state.clone();
        let im_context = im_context.clone();
        move |area| {
            if area.has_focus() {
                im_context.focus_in();
                update_im_cursor_location(area, &state, &im_context);
            } else {
                im_context.focus_out();
                state.preedit.borrow_mut().clear();
                area.queue_render();
            }
        }
    });
    keys.connect_im_update({
        let area = area.clone();
        let state = state.clone();
        let im_context = im_context.clone();
        move |_| update_im_cursor_location(&area, &state, &im_context)
    });
    keys.set_im_context(Some(&im_context));
}

fn update_im_cursor_location(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    im_context: &gtk::IMMulticontext,
) {
    let Some((x, y, width, height)) = render::cursor_rect(area, state) else {
        return;
    };
    im_context.set_cursor_location(&gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        width.ceil().max(1.0) as i32,
        height.ceil().max(1.0) as i32,
    ));
}

fn set_drag_selection(area: &gtk::GLArea, state: &Rc<EditorState>, anchor: usize, focus: usize) {
    state.selection.replace(Some(Selection {
        anchor,
        focus,
        visual_anchor: anchor,
        visual_focus: focus,
    }));
    state.cursor.set(focus);
    render::ensure_offset_visible(area, state, focus);
    reset_cursor_blink(area, state);
}

fn set_initial_drag_selection(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    offset: usize,
    selection: AnchoredSelection<usize>,
) {
    state.selection.replace(Some(Selection {
        anchor: offset,
        focus: offset,
        visual_anchor: selection.anchor,
        visual_focus: selection.focus,
    }));
    state.cursor.set(selection.focus);
    render::ensure_offset_visible(area, state, selection.focus);
    reset_cursor_blink(area, state);
}

fn apply_drag_selection(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    drag: DragSelection<usize>,
    focus: usize,
) {
    if let DragSelection::Character { anchor } = drag {
        set_drag_selection(area, state, anchor, focus);
        return;
    }

    let selection = state.selection.borrow();
    let (raw_anchor, raw_focus) = match *selection {
        Some(selection) => (selection.anchor, focus),
        None => (focus, focus),
    };
    drop(selection);

    let visual = selection_for_drag(
        drag,
        focus,
        |focus| word_drag_bounds(state, focus),
        |focus| line_drag_bounds(state, focus),
    );
    state.selection.replace(Some(Selection {
        anchor: raw_anchor,
        focus: raw_focus,
        visual_anchor: visual.anchor,
        visual_focus: visual.focus,
    }));
    state.cursor.set(visual.focus);
    render::ensure_offset_visible(area, state, visual.focus);
    reset_cursor_blink(area, state);
}

fn scroll_for_drag_selection(area: &gtk::GLArea, state: &Rc<EditorState>, pointer_y: f64) -> bool {
    let viewport_height = area.allocated_height().max(1) as f64;
    let line_height = render::line_height(state);
    let zone = line_height * super::super::drag_autoscroll::ZONE_LINES;
    if zone <= f64::EPSILON || viewport_height <= f64::EPSILON {
        return false;
    }
    let before = state.scroll_y.get();

    if pointer_y < 0.0 {
        let overflow = -pointer_y;
        let lines_per_frame = super::super::drag_autoscroll::lines_per_frame(overflow / zone);
        let delta = -(line_height * lines_per_frame);
        render::set_scroll_y(&area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    if pointer_y > viewport_height {
        let overflow = pointer_y - viewport_height;
        let lines_per_frame = super::super::drag_autoscroll::lines_per_frame(overflow / zone);
        let delta = line_height * lines_per_frame;
        render::set_scroll_y(&area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    false
}

fn schedule_drag_autoscroll(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    drag_autoscroll_id: &Rc<Cell<u64>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
    selection_drag: &Rc<Cell<Option<DragSelection<usize>>>>,
    selected_text_drag: &Rc<Cell<Option<SelectedTextDrag>>>,
    pointer_x: f64,
    pointer_y: f64,
    should_scroll: bool,
) {
    if should_scroll {
        drag_autoscroll_pointer.set(Some((pointer_x, pointer_y)));

        if drag_autoscroll_id.get() != 0 {
            return;
        }

        let next_id = drag_autoscroll_id.get().wrapping_add(1).max(1);
        drag_autoscroll_id.set(next_id);

        let area = area.clone();
        let state = state.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        let selection_drag = selection_drag.clone();
        let selected_text_drag = selected_text_drag.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
            if drag_autoscroll_id.get() != next_id {
                return gtk::glib::ControlFlow::Break;
            }

            let Some((x, y)) = drag_autoscroll_pointer.get() else {
                drag_autoscroll_id.set(0);
                return gtk::glib::ControlFlow::Break;
            };

            if !scroll_for_drag_selection(&area, &state, y) {
                drag_autoscroll_id.set(0);
                return gtk::glib::ControlFlow::Break;
            }

            if let Some(drag) = selection_drag.get() {
                let focus = render::hit_test(&area, &state, x, y);
                apply_drag_selection(&area, &state, drag, focus);
                return gtk::glib::ControlFlow::Continue;
            }

            if selected_text_drag.get().is_some() {
                update_selected_text_drag_drop(&area, &state, &selected_text_drag, x, y);
                return gtk::glib::ControlFlow::Continue;
            }

            drag_autoscroll_id.set(0);
            gtk::glib::ControlFlow::Break
        });
        return;
    }

    stop_drag_autoscroll(drag_autoscroll_id, drag_autoscroll_pointer);
}

fn begin_selected_text_drag(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    selection_drag: &Rc<Cell<Option<DragSelection<usize>>>>,
    selected_text_drag: &Rc<Cell<Option<SelectedTextDrag>>>,
    start: usize,
    end: usize,
    drop_offset: usize,
) {
    selection_drag.set(None);
    selected_text_drag.set(Some(SelectedTextDrag {
        start,
        end,
        drop_offset,
        active: false,
        before_cursor: state.cursor.get(),
        before_selection: *state.selection.borrow(),
    }));
    state.cursor.set(drop_offset);
    reset_cursor_blink(area, state);
}

fn update_selected_text_drag_drop(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    selected_text_drag: &Rc<Cell<Option<SelectedTextDrag>>>,
    x: f64,
    y: f64,
) {
    let Some(mut drag) = selected_text_drag.get() else {
        return;
    };
    let drop_offset = render::hit_test(area, state, x, y);
    drag.drop_offset = drop_offset;
    drag.active = true;
    selected_text_drag.set(Some(drag));
    state.cursor.set(drop_offset);
    render::ensure_offset_visible(area, state, drop_offset);
    reset_cursor_blink(area, state);
}

fn move_selected_text(area: &gtk::GLArea, state: &Rc<EditorState>, drag: SelectedTextDrag) {
    if !can_drag_move_selected_text(state) {
        restore_selected_text_drag_origin(area, state, drag);
        return;
    }

    let text = state.text.borrow();
    let start = previous_char_boundary(&text, drag.start.min(text.len()));
    let end = previous_char_boundary(&text, drag.end.min(text.len()).max(start));
    let drop_offset = previous_char_boundary(&text, drag.drop_offset.min(text.len()));
    if start == end || (start..=end).contains(&drop_offset) {
        drop(text);
        restore_selected_text_drag_origin(area, state, drag);
        return;
    }

    let selected = text[start..end].to_string();
    let selected_len = selected.len();
    let (replace_start, replace_end, replacement, moved_start, moved_end) = if drop_offset < start {
        let replacement = format!("{selected}{}", &text[drop_offset..start]);
        (
            drop_offset,
            end,
            replacement,
            drop_offset,
            drop_offset + selected_len,
        )
    } else {
        let moved_start = drop_offset - selected_len;
        let replacement = format!("{}{selected}", &text[end..drop_offset]);
        (start, drop_offset, replacement, moved_start, drop_offset)
    };
    drop(text);

    let moved_selection = Some(Selection {
        anchor: moved_start,
        focus: moved_end,
        visual_anchor: moved_start,
        visual_focus: moved_end,
    });
    state.cursor.set(drag.before_cursor);
    state.selection.replace(drag.before_selection);
    commit_edit(
        area,
        state,
        replace_start,
        replace_end,
        &replacement,
        moved_end,
        moved_selection,
        true,
    );
}

fn restore_selected_text_drag_origin(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    drag: SelectedTextDrag,
) {
    let text_len = state.text.borrow().len();
    let cursor = drag.before_cursor.min(text_len);
    state.cursor.set(cursor);
    state.selection.replace(drag.before_selection);
    render::ensure_offset_visible(area, state, cursor);
    reset_cursor_blink(area, state);
}

fn selected_text_drag_bounds_at(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    x: f64,
    y: f64,
) -> Option<(usize, usize)> {
    if !can_drag_move_selected_text(state) {
        return None;
    }
    let (text_start, text_end) = render::text_range_at_point(area, state, x, y)?;
    selection_bounds(state).filter(|(start, end)| text_start < *end && text_end > *start)
}

fn can_drag_move_selected_text(state: &Rc<EditorState>) -> bool {
    state.editable.get()
}

fn stop_drag_autoscroll(
    drag_autoscroll_id: &Rc<Cell<u64>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
) {
    drag_autoscroll_id.set(0);
    drag_autoscroll_pointer.set(None);
}

fn word_drag_bounds(state: &Rc<EditorState>, offset: usize) -> Option<(usize, usize)> {
    let text = state.text.borrow();
    word_bounds_at(&text, offset)
}

fn line_drag_bounds(state: &Rc<EditorState>, offset: usize) -> Option<(usize, usize)> {
    let text = state.text.borrow();
    Some(logical_line_bounds_at(&text, offset))
}

fn install_cursor_blink(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let area = area.downgrade();
    let state = state.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(530), move || {
        let Some(area) = area.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if area.has_focus() && state.editable.get() {
            state.cursor_visible.set(!state.cursor_visible.get());
            area.queue_render();
        } else if !state.cursor_visible.get() {
            state.cursor_visible.set(true);
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn reset_cursor_blink(area: &gtk::GLArea, state: &Rc<EditorState>) {
    state.cursor_visible.set(true);
    area.queue_render();
}

#[derive(Clone, Copy, Debug)]
struct SelectedTextDrag {
    start: usize,
    end: usize,
    drop_offset: usize,
    active: bool,
    before_cursor: usize,
    before_selection: Option<Selection>,
}

fn update_pointer_cursor(area: &gtk::GLArea, state: &Rc<EditorState>, x: f64, y: f64) {
    if state.middle_autoscroll.is_active() {
        return;
    }

    let over_scrollbar = state.scrollbar_visible.get()
        && canvas_scrollbar::point_in_lane(
            area.allocated_width(),
            area.allocated_height(),
            state.content_height.get(),
            x,
        );
    let fold_hover = (!over_scrollbar)
        .then(|| render::fold_control_at_point(area, state, x, y))
        .flatten();
    canvas_scrollbar::set_hover(
        area,
        &state.scrollbar_hover,
        &state.scrollbar_active,
        &state.scrollbar_hover_progress,
        &state.scrollbar_animating,
        over_scrollbar,
    );
    set_fold_hover(area, state, fold_hover);
    area.set_cursor_from_name(if fold_hover.is_some() {
        Some("pointer")
    } else if !over_scrollbar {
        Some("text")
    } else {
        None
    });
}

fn set_fold_hover(area: &gtk::GLArea, state: &Rc<EditorState>, hovered: Option<FoldControlKey>) {
    if state.fold_hovered.get() == hovered {
        return;
    }
    state.fold_hovered.set(hovered);
    start_fold_hover_animation(area, state);
    area.queue_render();
}

fn set_fold_pressed(area: &gtk::GLArea, state: &Rc<EditorState>, pressed: Option<FoldControlKey>) {
    if state.fold_pressed.get() == pressed {
        return;
    }
    state.fold_pressed.set(pressed);
    start_fold_hover_animation(area, state);
    area.queue_render();
}

fn start_fold_hover_animation(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if state.fold_hover_animating.get() {
        return;
    }
    state.fold_hover_animating.set(true);

    let area = area.clone();
    let state = state.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let target = if state.fold_hovered.get().is_some() || state.fold_pressed.get().is_some() {
            1.0
        } else {
            0.0
        };
        let current = state.fold_hover_progress.get();
        let delta = target - current;

        if delta.abs() < 0.02 {
            state.fold_hover_progress.set(target);
            state.fold_hover_animating.set(false);
            area.queue_render();
            return gtk::glib::ControlFlow::Break;
        }

        state.fold_hover_progress.set(current + delta * 0.32);
        area.queue_render();
        gtk::glib::ControlFlow::Continue
    });
}

#[derive(Clone, Copy, Debug)]
struct ClickPressState {
    count: i32,
    time: u32,
    x: f64,
    y: f64,
}

impl ClickPressState {
    const MAX_INTERVAL_MS: u32 = 500;
    const MAX_DISTANCE: f64 = 8.0;

    fn advance(self, time: u32, x: f64, y: f64) -> Self {
        let within_interval =
            self.count > 0 && time.wrapping_sub(self.time) <= Self::MAX_INTERVAL_MS;
        let within_distance =
            (x - self.x).abs() <= Self::MAX_DISTANCE && (y - self.y).abs() <= Self::MAX_DISTANCE;
        Self {
            count: if within_interval && within_distance {
                self.count + 1
            } else {
                1
            },
            time,
            x,
            y,
        }
    }

    fn selection_mode(self) -> SelectionMode {
        match (self.count.max(1) - 1).rem_euclid(3) {
            0 => SelectionMode::Character,
            1 => SelectionMode::Word,
            _ => SelectionMode::Line,
        }
    }
}

impl Default for ClickPressState {
    fn default() -> Self {
        Self {
            count: 0,
            time: 0,
            x: 0.0,
            y: 0.0,
        }
    }
}

fn handle_key(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    key: gdk::Key,
    modifiers: gdk::ModifierType,
) -> gtk::glib::Propagation {
    let command = modifiers.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::META_MASK
            | gdk::ModifierType::SUPER_MASK,
    );
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

    if handle_completion_key(area, state, key, command, shift) {
        return gtk::glib::Propagation::Stop;
    }

    if command {
        if ctrl && matches!(key, gdk::Key::z | gdk::Key::Z) {
            run_action(
                area,
                state,
                if shift {
                    TextContextAction::Redo
                } else {
                    TextContextAction::Undo
                },
            );
            return gtk::glib::Propagation::Stop;
        }
        if ctrl && matches!(key, gdk::Key::y | gdk::Key::Y) {
            run_action(area, state, TextContextAction::Redo);
            return gtk::glib::Propagation::Stop;
        }
        if ctrl && !shift && matches!(key, gdk::Key::slash | gdk::Key::KP_Divide) {
            toggle_line_comment(area, state);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(key, gdk::Key::a | gdk::Key::A) {
            run_action(area, state, TextContextAction::SelectAll);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(key, gdk::Key::c | gdk::Key::C) || (ctrl && key == gdk::Key::Insert) {
            run_action(area, state, TextContextAction::Copy);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(key, gdk::Key::x | gdk::Key::X) {
            run_action(area, state, TextContextAction::Cut);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(key, gdk::Key::v | gdk::Key::V) {
            run_action(area, state, TextContextAction::Paste);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(key, gdk::Key::w | gdk::Key::W) {
            run_action(area, state, TextContextAction::ToggleWrap);
            return gtk::glib::Propagation::Stop;
        }
    }

    if command && !ctrl {
        return gtk::glib::Propagation::Proceed;
    }

    if alt {
        return gtk::glib::Propagation::Proceed;
    }

    if key == gdk::Key::Escape {
        if clear_transient_selection(area, state) {
            return gtk::glib::Propagation::Stop;
        }
        return gtk::glib::Propagation::Proceed;
    }

    if shift && !ctrl && key == gdk::Key::Insert {
        run_action(area, state, TextContextAction::Paste);
        return gtk::glib::Propagation::Stop;
    }

    if key == gdk::Key::Page_Down {
        if state.editable.get() || shift {
            move_cursor_vertical(area, state, page_line_delta(area, state, 1), shift);
        } else {
            scroll_page(area, state, 1);
        }
        return gtk::glib::Propagation::Stop;
    }
    if key == gdk::Key::Page_Up {
        if state.editable.get() || shift {
            move_cursor_vertical(area, state, page_line_delta(area, state, -1), shift);
        } else {
            scroll_page(area, state, -1);
        }
        return gtk::glib::Propagation::Stop;
    }
    if !state.editable.get() {
        if command && !ctrl {
            return gtk::glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::Left => {
                let target = cursor_left_target(state, ctrl, shift);
                move_cursor_to(area, state, target, shift);
                return gtk::glib::Propagation::Stop;
            }
            gdk::Key::Right => {
                let target = cursor_right_target(state, ctrl, shift);
                move_cursor_to(area, state, target, shift);
                return gtk::glib::Propagation::Stop;
            }
            gdk::Key::Up => {
                move_cursor_vertical(area, state, -1, shift);
                return gtk::glib::Propagation::Stop;
            }
            gdk::Key::Down => {
                move_cursor_vertical(area, state, 1, shift);
                return gtk::glib::Propagation::Stop;
            }
            gdk::Key::Home => {
                let target = if ctrl {
                    0
                } else {
                    smart_home_target(&state.text.borrow(), state.cursor.get())
                };
                move_cursor_to(area, state, target, shift);
                return gtk::glib::Propagation::Stop;
            }
            gdk::Key::End => {
                let target = if ctrl {
                    state.text.borrow().len()
                } else {
                    current_line_end(&state.text.borrow(), state.cursor.get())
                };
                move_cursor_to(area, state, target, shift);
                return gtk::glib::Propagation::Stop;
            }
            _ => {}
        }
        return gtk::glib::Propagation::Proceed;
    }
    match key {
        gdk::Key::Left => {
            let target = cursor_left_target(state, ctrl, shift);
            move_cursor_to(area, state, target, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::Right => {
            let target = cursor_right_target(state, ctrl, shift);
            move_cursor_to(area, state, target, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::Up => {
            move_cursor_vertical(area, state, -1, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::Down => {
            move_cursor_vertical(area, state, 1, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::Home => {
            let target = if ctrl {
                0
            } else {
                smart_home_target(&state.text.borrow(), state.cursor.get())
            };
            move_cursor_to(area, state, target, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::End => {
            let target = if ctrl {
                state.text.borrow().len()
            } else {
                current_line_end(&state.text.borrow(), state.cursor.get())
            };
            move_cursor_to(area, state, target, shift);
            return gtk::glib::Propagation::Stop;
        }
        gdk::Key::Delete => {
            let deleted_leading_whitespace = shift && edit_delete_leading_whitespace(area, state);
            if !deleted_leading_whitespace {
                if ctrl {
                    edit_delete_word(area, state, DeleteDirection::Forward, shift);
                } else if shift && selection_bounds(state).is_some() {
                    run_action(area, state, TextContextAction::Cut);
                } else {
                    edit_delete(area, state);
                }
            }
            request_or_dismiss_completion(area, state);
            return gtk::glib::Propagation::Stop;
        }
        _ => {}
    }
    if key == gdk::Key::BackSpace {
        if ctrl {
            edit_delete_word(area, state, DeleteDirection::Backward, shift);
        } else {
            edit_backspace(area, state);
        }
        request_or_dismiss_completion(area, state);
        return gtk::glib::Propagation::Stop;
    }
    if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
        insert_newline(area, state);
        dismiss_completion(state);
        return gtk::glib::Propagation::Stop;
    }
    if is_tab_key(key) && !command {
        edit_tab(area, state, shift || key == gdk::Key::ISO_Left_Tab);
        dismiss_completion(state);
        return gtk::glib::Propagation::Stop;
    }
    if !command {
        if let Some(ch) = key.to_unicode().filter(|ch| !ch.is_control()) {
            if let Some(close) = matching_auto_pair(ch) {
                insert_auto_pair(area, state, ch, close);
            } else {
                insert_text(area, state, &ch.to_string());
            }
            request_or_dismiss_completion(area, state);
            return gtk::glib::Propagation::Stop;
        }
    }
    gtk::glib::Propagation::Proceed
}
