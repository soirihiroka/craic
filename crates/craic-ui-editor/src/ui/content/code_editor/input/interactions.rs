pub fn install_interactions(area: &gtk::GLArea, root: &gtk::Box, state: &Rc<EditorState>) {
    let scroll_drag = Rc::new(Cell::new(None::<canvas_scrollbar::Drag>));
    let selection_drag = Rc::new(Cell::new(None::<DragSelection<usize>>));
    let selected_text_drag = Rc::new(Cell::new(None::<SelectedTextDrag>));
    let pending_selection_click = Rc::new(Cell::new(None::<usize>));
    let click_press_state = Rc::new(Cell::new(ClickPressState::default()));
    let drag_autoscroll_id = Rc::new(Cell::new(0_u64));
    let drag_autoscroll_pointer = Rc::new(Cell::new(None::<(f64, f64)>));

    install_cursor_blink(area, state);
    install_editor_middle_autoscroll(area, state);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let root = root.to_owned();
    scroll.connect_scroll({
        let area = area.clone();
        let state = state.clone();
        let root = root.clone();
        move |controller, dx, dy| {
            let modifiers = controller.current_event_state();
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                && !modifiers.contains(gdk::ModifierType::ALT_MASK)
            {
                let delta = if dy.abs() >= dx.abs() {
                    if dy < 0.0 { 1.0 } else { -1.0 }
                } else if dx < 0.0 {
                    1.0
                } else {
                    -1.0
                };
                let next = super::set_font_size_for_state(
                    &area,
                    &root,
                    &state,
                    state.font_size.get() + delta,
                );
                config::save_editor_font_size(next);
                return gtk::glib::Propagation::Stop;
            }
            if dy.abs() > f64::EPSILON {
                let line_height = render::line_height(&state);
                let viewport_height = area.allocated_height().max(1) as f64;
                let max_scroll = render::max_scroll_y(&state, viewport_height);
                if state.scrollbar_visible.get()
                    && state.scrollbar_hover.get()
                    && canvas_scrollbar::is_mouse_scroll(controller)
                {
                    let delta = canvas_scrollbar::mouse_wheel_delta(viewport_height, dy);
                    canvas_overshoot::pull_for_delta(
                        &area,
                        &state.overshoot,
                        state.scroll_y.get(),
                        max_scroll,
                        delta,
                        canvas_overshoot::Edge::Top,
                        canvas_overshoot::Edge::Bottom,
                    );
                    let area_for_scroll = area.clone();
                    let state_for_scroll = state.clone();
                    state.scrollbar_smooth_scroll.scroll_relative(
                        &area,
                        state.scroll_y.get(),
                        delta,
                        0.0,
                        max_scroll,
                        move |value| {
                            render::set_scroll_y(&area_for_scroll, &state_for_scroll, value)
                        },
                    );
                    return gtk::glib::Propagation::Stop;
                }

                state.scrollbar_smooth_scroll.pause();
                let delta = dy * line_height * 3.0;
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
            if dx.abs() > f64::EPSILON {
                state.scrollbar_smooth_scroll.pause();
                let line_height = render::line_height(&state);
                let delta = dx * line_height * 3.0;
                let viewport_width = render::viewport_width(area.allocated_width()) as f64;
                canvas_overshoot::pull_for_delta(
                    &area,
                    &state.overshoot,
                    state.scroll_x.get(),
                    (state.content_width.get() - viewport_width).max(0.0),
                    delta,
                    canvas_overshoot::Edge::Left,
                    canvas_overshoot::Edge::Right,
                );
                render::set_scroll_x(&area, &state, state.scroll_x.get() + delta);
            }
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(scroll);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter({
        let area = area.clone();
        let state = state.clone();

        move |_, x, y| {
            update_pointer_cursor(&area, &state, x, y);
        }
    });
    motion.connect_motion({
        let area = area.clone();
        let state = state.clone();

        move |_, x, y| {
            update_pointer_cursor(&area, &state, x, y);
        }
    });
    motion.connect_leave({
        let area = area.clone();
        let state = state.clone();

        move |_| {
            if state.middle_autoscroll.is_active() {
                clear_editor_autoscroll_hover(&area, &state);
                return;
            }

            canvas_scrollbar::set_hover(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                false,
            );
            set_fold_hover(&area, &state, None);
            set_fold_pressed(&area, &state, None);
            area.set_cursor_from_name(None);
        }
    });
    area.add_controller(motion);

    area.connect_has_focus_notify({
        let area = area.clone();
        let state = state.clone();
        move |_| reset_cursor_blink(&area, &state)
    });

    let press = gtk::EventControllerLegacy::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    press.connect_event({
        let area = area.clone();
        let state = state.clone();
        let click_press_state = click_press_state.clone();

        move |_, event| {
            if event.event_type() != gdk::EventType::ButtonPress {
                return gtk::glib::Propagation::Proceed;
            }
            let Some(button) = event.downcast_ref::<gdk::ButtonEvent>() else {
                return gtk::glib::Propagation::Proceed;
            };
            if button.button() != 1 {
                return gtk::glib::Propagation::Proceed;
            }
            let Some((x, y)) = event.position() else {
                return gtk::glib::Propagation::Proceed;
            };

            let width = area.allocated_width();
            let height = area.allocated_height();
            let total_height = state.content_height.get();
            if state.scrollbar_visible.get()
                && canvas_scrollbar::scroll_for_lane_press(
                    width,
                    height,
                    total_height,
                    state.scroll_y.get(),
                    x,
                    y,
                )
                .is_some()
            {
                log::debug!("code_editor raw_button_press ignored=scrollbar x={x:.1} y={y:.1}");
                return gtk::glib::Propagation::Proceed;
            }
            if render::fold_control_at_point(&area, &state, x, y).is_some() {
                log::debug!("code_editor raw_button_press ignored=fold x={x:.1} y={y:.1}");
                return gtk::glib::Propagation::Proceed;
            }

            let next = click_press_state.get().advance(event.time(), x, y);
            click_press_state.set(next);
            let mode = next.selection_mode();
            log::debug!(
                "code_editor raw_button_press count={} mode={mode:?} time={} x={x:.1} y={y:.1}",
                next.count,
                event.time(),
            );
            gtk::glib::Propagation::Proceed
        }
    });
    area.add_controller(press);

    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.connect_pressed({
        let area = area.clone();
        let state = state.clone();
        let click_press_state = click_press_state.clone();
        let pending_selection_click = pending_selection_click.clone();
        move |gesture, n_press, x, y| {
            area.grab_focus();
            let button = gesture.current_button();
            if button == 2 {
                pending_selection_click.set(None);
                set_fold_pressed(&area, &state, None);
                return;
            }
            if state.middle_autoscroll.is_active() {
                pending_selection_click.set(None);
                set_fold_pressed(&area, &state, None);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            dismiss_completion(&state);
            if button == 3 {
                pending_selection_click.set(None);
                set_fold_pressed(&area, &state, None);
                position_context_click(&area, &state, x, y);
                show_context_menu(&area, &state, x, y);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            let width = area.allocated_width();
            let height = area.allocated_height();
            let total_height = state.content_height.get();
            if state.scrollbar_visible.get() {
                if let Some(scroll_y) = canvas_scrollbar::scroll_for_lane_press(
                    width,
                    height,
                    total_height,
                    state.scroll_y.get(),
                    x,
                    y,
                ) {
                    log::debug!(
                        "code_editor click_pressed scrollbar button={} n_press={} x={x:.1} y={y:.1} scroll_y={scroll_y:.1}",
                        gesture.current_button(),
                        n_press,
                    );
                    pending_selection_click.set(None);
                    set_fold_pressed(&area, &state, None);
                    state.scrollbar_smooth_scroll.pause();
                    render::set_scroll_y(&area, &state, scroll_y);
                    return;
                }
            }
            set_fold_pressed(
                &area,
                &state,
                render::fold_control_at_point(&area, &state, x, y),
            );
            if toggle_fold_at(&area, &state, x, y) {
                log::debug!(
                    "code_editor click_pressed fold_toggle button={} n_press={} x={x:.1} y={y:.1}",
                    gesture.current_button(),
                    n_press,
                );
                pending_selection_click.set(None);
                return;
            }
            set_fold_pressed(&area, &state, None);
            let offset = render::hit_test(&area, &state, x, y);
            let mode = click_press_state.get().selection_mode();
            log::debug!(
                "code_editor click_pressed content button={} gtk_n_press={} mode={mode:?} offset={offset} x={x:.1} y={y:.1} selection_before={:?}",
                gesture.current_button(),
                n_press,
                *state.selection.borrow(),
            );
            if gesture.current_button() == 1
                && mode == SelectionMode::Character
                && selected_text_drag_bounds_at(&area, &state, x, y).is_some()
            {
                pending_selection_click.set(Some(offset));
                return;
            }
            pending_selection_click.set(None);
            match mode {
                SelectionMode::Character => {
                    move_cursor_to(&area, &state, offset, false);
                }
                SelectionMode::Word | SelectionMode::Line => {
                    if !select_at_mode(&area, &state, offset, mode) {
                        move_cursor_to(&area, &state, offset, false);
                    }
                }
            }
        }
    });
    click.connect_released({
        let area = area.clone();
        let state = state.clone();
        let click_press_state = click_press_state.clone();
        let pending_selection_click = pending_selection_click.clone();

        move |gesture, n_press, x, y| {
            if gesture.current_button() == 2 {
                set_fold_pressed(&area, &state, None);
                return;
            }

            log::debug!(
                "code_editor click_released n_press={} x={x:.1} y={y:.1} mode={:?} selection={:?}",
                n_press,
                click_press_state.get().selection_mode(),
                *state.selection.borrow(),
            );
            if pending_selection_click.take().is_some() {
                let offset = render::hit_test(&area, &state, x, y);
                move_cursor_to(&area, &state, offset, false);
            }
            set_fold_pressed(&area, &state, None);
        }
    });

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.connect_drag_begin({
        let area = area.clone();
        let state = state.clone();
        let scroll_drag = scroll_drag.clone();
        let selection_drag = selection_drag.clone();
        let selected_text_drag = selected_text_drag.clone();
        let pending_selection_click = pending_selection_click.clone();
        let click_press_state = click_press_state.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, x, y| {
            area.grab_focus();
            if state.middle_autoscroll.is_active() {
                pending_selection_click.set(None);
                set_fold_pressed(&area, &state, None);
                return;
            }
            dismiss_completion(&state);
            let width = area.allocated_width();
            let height = area.allocated_height();
            let total_height = state.content_height.get();
            if state.scrollbar_visible.get() {
                if let Some(scroll_y) = canvas_scrollbar::scroll_for_lane_press(
                    width,
                    height,
                    total_height,
                    state.scroll_y.get(),
                    x,
                    y,
                ) {
                    log::debug!(
                        "code_editor drag_begin scrollbar x={x:.1} y={y:.1} scroll_y={scroll_y:.1}",
                    );
                    state.scrollbar_smooth_scroll.pause();
                    render::set_scroll_y(&area, &state, scroll_y);
                    canvas_scrollbar::set_active(
                        &area,
                        &state.scrollbar_hover,
                        &state.scrollbar_active,
                        &state.scrollbar_hover_progress,
                        &state.scrollbar_animating,
                        true,
                    );
                    set_fold_pressed(&area, &state, None);
                    stop_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
                    scroll_drag.set(Some(canvas_scrollbar::Drag::new(state.scroll_y.get())));
                    selection_drag.set(None);
                    selected_text_drag.set(None);
                    pending_selection_click.set(None);
                    return;
                }
            }

            canvas_scrollbar::set_active(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                false,
            );
            scroll_drag.set(None);

            let fold_key = render::fold_control_at_point(&area, &state, x, y);
            set_fold_pressed(&area, &state, fold_key);
            if fold_key.is_some() {
                log::debug!("code_editor drag_begin fold x={x:.1} y={y:.1}");
                stop_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
                selection_drag.set(None);
                selected_text_drag.set(None);
                pending_selection_click.set(None);
                return;
            }

            let offset = render::hit_test(&area, &state, x, y);
            let mode = click_press_state.get().selection_mode();
            log::debug!(
                "code_editor drag_begin content mode={mode:?} offset={offset} x={x:.1} y={y:.1} selection_before={:?}",
                *state.selection.borrow(),
            );
            if mode == SelectionMode::Character
                && selected_text_drag_bounds_at(&area, &state, x, y).is_some()
            {
                selection_drag.set(None);
                selected_text_drag.set(None);
                return;
            }
            pending_selection_click.set(None);
            selected_text_drag.set(None);
            let (drag, selection) = drag_for_mode(
                offset,
                mode,
                |offset| word_drag_bounds(&state, offset),
                |offset| line_drag_bounds(&state, offset),
            );
            selection_drag.set(Some(drag));
            set_initial_drag_selection(&area, &state, offset, selection);
        }
    });
    drag.connect_drag_update({
        let area = area.clone();
        let state = state.clone();
        let scroll_drag = scroll_drag.clone();
        let selection_drag = selection_drag.clone();
        let selected_text_drag = selected_text_drag.clone();
        let pending_selection_click = pending_selection_click.clone();
        let click_press_state = click_press_state.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();

        move |gesture, offset_x, offset_y| {
            if let Some(drag) = scroll_drag.get() {
                let Some((_, _, _, thumb_height)) = render::scrollbar_thumb(&area, &state) else {
                    return;
                };
                let viewport_height = area.allocated_height().max(1) as f64;
                let max_scroll = render::max_scroll_y(&state, viewport_height);
                state.scrollbar_smooth_scroll.pause();
                render::set_scroll_y(
                    &area,
                    &state,
                    drag.scroll_for_delta(offset_y, viewport_height, thumb_height, max_scroll),
                );
                return;
            }
            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            if selection_drag.get().is_none()
                && selected_text_drag.get().is_none()
                && render::fold_action_at_point(&area, &state, start_x, start_y).is_some()
            {
                stop_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
                return;
            }
            let pointer_x = start_x + offset_x;
            let pointer_y = start_y + offset_y;
            let should_autoscroll = scroll_for_drag_selection(&area, &state, pointer_y);
            if selected_text_drag.get().is_some() {
                pending_selection_click.set(None);
                update_selected_text_drag_drop(
                    &area,
                    &state,
                    &selected_text_drag,
                    pointer_x,
                    pointer_y,
                );
                schedule_drag_autoscroll(
                    &area,
                    &state,
                    &drag_autoscroll_id,
                    &drag_autoscroll_pointer,
                    &selection_drag,
                    &selected_text_drag,
                    pointer_x,
                    pointer_y,
                    should_autoscroll,
                );
                return;
            }
            let anchor = render::hit_test(&area, &state, start_x, start_y);
            // A selection drag can grow back over its start point while autoscrolling.
            // Only unclaimed drags should promote to moving selected text.
            if selection_drag.get().is_none()
                && click_press_state.get().selection_mode() == SelectionMode::Character
            {
                if let Some((start, end)) =
                    selected_text_drag_bounds_at(&area, &state, start_x, start_y)
                {
                    pending_selection_click.set(None);
                    begin_selected_text_drag(
                        &area,
                        &state,
                        &selection_drag,
                        &selected_text_drag,
                        start,
                        end,
                        anchor,
                    );
                    update_selected_text_drag_drop(
                        &area,
                        &state,
                        &selected_text_drag,
                        pointer_x,
                        pointer_y,
                    );
                    schedule_drag_autoscroll(
                        &area,
                        &state,
                        &drag_autoscroll_id,
                        &drag_autoscroll_pointer,
                        &selection_drag,
                        &selected_text_drag,
                        pointer_x,
                        pointer_y,
                        should_autoscroll,
                    );
                    return;
                }
            }
            let drag = selection_drag
                .get()
                .unwrap_or(DragSelection::Character { anchor });
            let focus = render::hit_test(&area, &state, pointer_x, pointer_y);
            selection_drag.set(Some(drag));
            log::debug!(
                "code_editor drag_update start=({start_x:.1},{start_y:.1}) delta=({offset_x:.1},{offset_y:.1}) focus={focus} drag={drag:?} selection_before={:?}",
                *state.selection.borrow(),
            );
            apply_drag_selection(&area, &state, drag, focus);
            schedule_drag_autoscroll(
                &area,
                &state,
                &drag_autoscroll_id,
                &drag_autoscroll_pointer,
                &selection_drag,
                &selected_text_drag,
                pointer_x,
                pointer_y,
                should_autoscroll,
            );
        }
    });
    drag.connect_drag_end({
        let area = area.clone();
        let state = state.clone();
        let scroll_drag = scroll_drag.clone();
        let selection_drag = selection_drag.clone();
        let selected_text_drag = selected_text_drag.clone();
        let pending_selection_click = pending_selection_click.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, _, _| {
            log::debug!(
                "code_editor drag_end scroll_drag={} selection_drag={:?} selected_text_drag={:?}",
                scroll_drag.get().is_some(),
                selection_drag.get(),
                selected_text_drag.get(),
            );
            if let Some(drag) = selected_text_drag.get().filter(|drag| drag.active) {
                move_selected_text(&area, &state, drag);
                pending_selection_click.set(None);
            }
            scroll_drag.set(None);
            selection_drag.set(None);
            selected_text_drag.set(None);
            stop_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
            canvas_scrollbar::set_active(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                false,
            );
            set_fold_pressed(&area, &state, None);
        }
    });

    click.group_with(&drag);
    area.add_controller(click);
    area.add_controller(drag);

    let keys = gtk::EventControllerKey::new();
    install_im_context(area, state, &keys);
    keys.connect_key_pressed({
        let area = area.clone();
        let state = state.clone();
        move |_, key, _, modifiers| {
            if key == gdk::Key::Escape && state.middle_autoscroll.is_active() {
                return gtk::glib::Propagation::Stop;
            }
            handle_key(&area, &state, key, modifiers)
        }
    });
    area.add_controller(keys);
}
