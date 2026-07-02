use super::super::canvas_overshoot;
use super::selection::word_bounds_at;
use super::{
    CompletionUi, EditorState, FoldControlKey, FoldRange, HistorySnapshot, MAX_HISTORY_SNAPSHOTS,
    Selection, SelectionMode, notify_diff_fold, notify_edit, render, selection_bounds,
};
use crate::config;
use crate::language_support::{CompletionSet, NewlineContext, enter_newline};
use crate::spellcheck::SpellcheckIssue;
use crate::ui::components::context_menu::{
    self, MenuActionState, TextContextAction, TextContextMenuState,
};
use crate::ui::{canvas_scroll, canvas_scrollbar};
use adw::prelude::*;
use gtk::gdk;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

const DRAG_AUTOSCROLL_ZONE_LINES: f64 = 2.0;
const DRAG_AUTOSCROLL_MIN_LINES_PER_FRAME: f64 = 0.5;
const DRAG_AUTOSCROLL_BASE_EXTRA_LINES_PER_FRAME: f64 = 1.5;
const DRAG_AUTOSCROLL_AGGRESSIVE_EXTRA_LINES_PER_FRAME: f64 = 2.0;
const DRAG_AUTOSCROLL_OUTSIDE_EXTRA_LINES_PER_FRAME: f64 = 2.0;
const INDENT_TEXT: &str = "    ";

pub(super) fn install_interactions(
    area: &gtk::DrawingArea,
    root: &gtk::Box,
    state: &Rc<EditorState>,
) {
    let scroll_drag = Rc::new(Cell::new(None::<canvas_scrollbar::Drag>));
    let selection_drag = Rc::new(Cell::new(None::<DragSelection>));
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
                if let Some(callback) = state.font_size_adjust_callback.borrow().clone() {
                    callback(delta);
                } else {
                    let next = super::set_font_size_for_state(
                        &area,
                        &root,
                        &state,
                        state.font_size.get() + delta,
                    );
                    config::save_editor_font_size(next);
                }
                return gtk::glib::Propagation::Stop;
            }
            if dy.abs() > f64::EPSILON {
                let line_height = render::line_height(&state);
                let delta = dy * line_height * 3.0;
                let viewport_height = area.allocated_height().max(1) as f64;
                canvas_overshoot::pull_for_delta(
                    &area,
                    &state.overshoot,
                    state.scroll_y.get(),
                    render::max_scroll_y(&state, viewport_height),
                    delta,
                    canvas_overshoot::Edge::Top,
                    canvas_overshoot::Edge::Bottom,
                );
                render::set_scroll_y(&area, &state, state.scroll_y.get() + delta);
            }
            if dx.abs() > f64::EPSILON {
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
                SelectionMode::Word => {
                    if !select_word_at(&area, &state, offset) {
                        move_cursor_to(&area, &state, offset, false);
                    }
                }
                SelectionMode::Line => {
                    select_line_at(&area, &state, offset);
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
            match mode {
                SelectionMode::Line => {
                    if let Some((start, end)) = line_drag_bounds(&state, offset) {
                        select_line_at(&area, &state, offset);
                        selection_drag.set(Some(DragSelection::Line { start, end }));
                        reset_cursor_blink(&area, &state);
                    } else {
                        selection_drag.set(Some(DragSelection::Character { anchor: offset }));
                        set_drag_selection(&area, &state, offset, offset);
                    }
                }
                SelectionMode::Word => {
                    if let Some((start, end)) = word_drag_bounds(&state, offset) {
                        select_word_at(&area, &state, offset);
                        selection_drag.set(Some(DragSelection::Word { start, end }));
                        reset_cursor_blink(&area, &state);
                    } else {
                        selection_drag.set(Some(DragSelection::Character { anchor: offset }));
                        set_drag_selection(&area, &state, offset, offset);
                    }
                }
                SelectionMode::Character => {
                    selection_drag.set(Some(DragSelection::Character { anchor: offset }));
                    set_drag_selection(&area, &state, offset, offset);
                }
            }
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
            let drag = selection_drag.get().unwrap_or_else(|| {
                DragSelection::Character { anchor }
            });
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

fn install_editor_middle_autoscroll(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
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
            move || area.queue_draw()
        },
    );
}

fn clear_editor_autoscroll_hover(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
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

fn install_im_context(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    keys: &gtk::EventControllerKey,
) {
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
            area.queue_draw();
        }
    });
    im_context.connect_preedit_end({
        let area = area.clone();
        let state = state.clone();
        move |_| {
            state.preedit.borrow_mut().clear();
            area.queue_draw();
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
                area.queue_draw();
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
    area: &gtk::DrawingArea,
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

fn set_drag_selection(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    anchor: usize,
    focus: usize,
) {
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

fn apply_drag_selection(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    drag: DragSelection,
    focus: usize,
) {
    match drag {
        DragSelection::Character { anchor } => set_drag_selection(area, state, anchor, focus),
        DragSelection::Word { start, end } => {
            set_word_drag_selection(area, state, start, end, focus)
        }
        DragSelection::Line { start, end } => {
            set_line_drag_selection(area, state, start, end, focus)
        }
    }
}

fn scroll_for_drag_selection(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    pointer_y: f64,
) -> bool {
    let viewport_height = area.allocated_height().max(1) as f64;
    let line_height = render::line_height(state);
    let zone = line_height * DRAG_AUTOSCROLL_ZONE_LINES;
    if zone <= f64::EPSILON || viewport_height <= f64::EPSILON {
        return false;
    }
    let before = state.scroll_y.get();

    if pointer_y < 0.0 {
        let overflow = -pointer_y;
        let lines_per_frame = drag_autoscroll_lines_per_frame(overflow / zone);
        let delta = -(line_height * lines_per_frame);
        render::set_scroll_y(&area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    if pointer_y > viewport_height {
        let overflow = pointer_y - viewport_height;
        let lines_per_frame = drag_autoscroll_lines_per_frame(overflow / zone);
        let delta = line_height * lines_per_frame;
        render::set_scroll_y(&area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    false
}

fn drag_autoscroll_lines_per_frame(ratio: f64) -> f64 {
    let ramp_ratio = ratio.max(0.0).min(1.0);
    let outside_ratio = (ratio - 1.0).max(0.0);
    DRAG_AUTOSCROLL_MIN_LINES_PER_FRAME
        + ramp_ratio * DRAG_AUTOSCROLL_BASE_EXTRA_LINES_PER_FRAME
        + ramp_ratio.powi(3) * DRAG_AUTOSCROLL_AGGRESSIVE_EXTRA_LINES_PER_FRAME
        + outside_ratio * DRAG_AUTOSCROLL_OUTSIDE_EXTRA_LINES_PER_FRAME
}

fn schedule_drag_autoscroll(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    drag_autoscroll_id: &Rc<Cell<u64>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
    selection_drag: &Rc<Cell<Option<DragSelection>>>,
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
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    selection_drag: &Rc<Cell<Option<DragSelection>>>,
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
    area: &gtk::DrawingArea,
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

fn move_selected_text(area: &gtk::DrawingArea, state: &Rc<EditorState>, drag: SelectedTextDrag) {
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
    area: &gtk::DrawingArea,
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
    area: &gtk::DrawingArea,
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
    state.editable.get() && state.diff_rows.borrow().is_none()
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

fn set_word_drag_selection(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    word_start: usize,
    word_end: usize,
    focus: usize,
) {
    let selection = state.selection.borrow();
    let (raw_anchor, raw_focus) = match *selection {
        Some(selection) => (selection.anchor, focus),
        None => (focus, focus),
    };
    drop(selection);

    let text = state.text.borrow();
    let focus = focus.min(text.len());
    let (visual_anchor, visual_focus) = if focus < word_start {
        let focus = word_bounds_at(&text, focus)
            .map(|(start, _)| start)
            .unwrap_or(focus);
        (word_end, focus)
    } else if focus > word_end {
        let focus = word_bounds_at(&text, focus)
            .map(|(_, end)| end)
            .unwrap_or(focus);
        (word_start, focus)
    } else {
        (word_start, word_end)
    };
    drop(text);

    state.selection.replace(Some(Selection {
        anchor: raw_anchor,
        focus: raw_focus,
        visual_anchor,
        visual_focus,
    }));
    state.cursor.set(visual_focus);
    render::ensure_offset_visible(area, state, visual_focus);
    reset_cursor_blink(area, state);
}

fn set_line_drag_selection(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    line_start: usize,
    line_end: usize,
    focus: usize,
) {
    let selection = state.selection.borrow();
    let (raw_anchor, raw_focus) = match *selection {
        Some(selection) => (selection.anchor, focus),
        None => (focus, focus),
    };
    drop(selection);

    let text = state.text.borrow();
    let focus = focus.min(text.len());
    let (visual_anchor, visual_focus) = if focus < line_start {
        let (focus_start, _) = logical_line_bounds_at(&text, focus);
        (line_end, focus_start)
    } else if focus > line_end {
        let (_, focus_end) = logical_line_bounds_at(&text, focus);
        (line_start, focus_end)
    } else {
        (line_start, line_end)
    };
    drop(text);

    state.selection.replace(Some(Selection {
        anchor: raw_anchor,
        focus: raw_focus,
        visual_anchor,
        visual_focus,
    }));
    state.cursor.set(visual_focus);
    render::ensure_offset_visible(area, state, visual_focus);
    reset_cursor_blink(area, state);
}

fn install_cursor_blink(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let area = area.downgrade();
    let state = state.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(530), move || {
        let Some(area) = area.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if area.has_focus() && state.editable.get() {
            state.cursor_visible.set(!state.cursor_visible.get());
            area.queue_draw();
        } else if !state.cursor_visible.get() {
            state.cursor_visible.set(true);
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn reset_cursor_blink(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    state.cursor_visible.set(true);
    area.queue_draw();
}

#[derive(Clone, Copy, Debug)]
enum DragSelection {
    Character { anchor: usize },
    Word { start: usize, end: usize },
    Line { start: usize, end: usize },
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

fn update_pointer_cursor(area: &gtk::DrawingArea, state: &Rc<EditorState>, x: f64, y: f64) {
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

fn set_fold_hover(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    hovered: Option<FoldControlKey>,
) {
    if state.fold_hovered.get() == hovered {
        return;
    }
    state.fold_hovered.set(hovered);
    start_fold_hover_animation(area, state);
    area.queue_draw();
}

fn set_fold_pressed(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    pressed: Option<FoldControlKey>,
) {
    if state.fold_pressed.get() == pressed {
        return;
    }
    state.fold_pressed.set(pressed);
    start_fold_hover_animation(area, state);
    area.queue_draw();
}

fn start_fold_hover_animation(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
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
            area.queue_draw();
            return gtk::glib::ControlFlow::Break;
        }

        state.fold_hover_progress.set(current + delta * 0.32);
        area.queue_draw();
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
    area: &gtk::DrawingArea,
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
            insert_text(area, state, &ch.to_string());
            request_or_dismiss_completion(area, state);
            return gtk::glib::Propagation::Stop;
        }
    }
    gtk::glib::Propagation::Proceed
}

fn clear_transient_selection(area: &gtk::DrawingArea, state: &Rc<EditorState>) -> bool {
    let cleared_selection = state.selection.borrow_mut().take().is_some();
    let cleared_preedit = {
        let mut preedit = state.preedit.borrow_mut();
        let cleared = !preedit.is_empty();
        if cleared {
            preedit.clear();
        }
        cleared
    };

    if !cleared_selection && !cleared_preedit {
        return false;
    }

    log::debug!(
        "code_editor escape cleared selection={cleared_selection} preedit={cleared_preedit}"
    );
    reset_cursor_blink(area, state);
    true
}

fn is_tab_key(key: gdk::Key) -> bool {
    matches!(key, gdk::Key::Tab | gdk::Key::ISO_Left_Tab)
}

fn edit_tab(area: &gtk::DrawingArea, state: &Rc<EditorState>, outdent: bool) {
    if outdent {
        edit_line_indentation(area, state, false);
    } else if selection_bounds(state).is_some() {
        edit_line_indentation(area, state, true);
    } else {
        insert_text(area, state, INDENT_TEXT);
    }
}

fn position_context_click(area: &gtk::DrawingArea, state: &Rc<EditorState>, x: f64, y: f64) {
    if render::fold_action_at_point(area, state, x, y).is_some() {
        return;
    }
    let offset = render::hit_test(area, state, x, y);
    let inside_selection =
        selection_bounds(state).is_some_and(|(start, end)| offset >= start && offset < end);
    if !inside_selection {
        move_cursor_to(area, state, offset, false);
    }
}

fn show_context_menu(area: &gtk::DrawingArea, state: &Rc<EditorState>, x: f64, y: f64) {
    let offset = render::hit_test(area, state, x, y);
    context_menu::popup_action_menu(area, x, y, editor_context_menu_sections(state, offset), {
        let area = area.clone();
        let state = state.clone();

        move |action| {
            run_action(&area, &state, action);
            area.grab_focus();
        }
    });
}

pub(super) fn apply_completion_result(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    completions: Option<CompletionSet>,
) {
    let Some(completions) = completions.filter(|completions| !completions.items.is_empty()) else {
        clear_completion(state, false);
        return;
    };

    if completions.replacement_end != state.cursor.get() {
        clear_completion(state, false);
        return;
    }

    {
        let mut completion = state.completion.borrow_mut();
        completion.items = completions.items;
        completion.selected = 0;
        completion.replacement_range =
            Some((completions.replacement_start, completions.replacement_end));
    }

    show_completion_popover(area, state);
}

pub(super) fn dismiss_completion(state: &Rc<EditorState>) {
    clear_completion(state, true);
}

fn handle_completion_key(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    key: gdk::Key,
    command: bool,
    shift: bool,
) -> bool {
    if !completion_is_open(state) || command {
        return false;
    }

    match key {
        gdk::Key::Escape => {
            dismiss_completion(state);
            true
        }
        gdk::Key::Down => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Up => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Left
        | gdk::Key::Right
        | gdk::Key::Home
        | gdk::Key::End
        | gdk::Key::Page_Up
        | gdk::Key::Page_Down => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Return | gdk::Key::KP_Enter => {
            accept_completion(area, state);
            true
        }
        gdk::Key::Tab if !shift => {
            accept_completion(area, state);
            true
        }
        _ => false,
    }
}

fn dismiss_completion_for_navigation(state: &Rc<EditorState>, key: gdk::Key) {
    log::debug!("code_editor completion dismissed for navigation key={key:?}");
    dismiss_completion(state);
}

fn completion_is_open(state: &Rc<EditorState>) -> bool {
    let completion = state.completion.borrow();
    !completion.items.is_empty() && completion.replacement_range.is_some()
}

fn request_or_dismiss_completion(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if !state.editable.get()
        || state.selection.borrow().is_some()
        || !matches!(state.language.borrow().as_str(), "rust" | "rs")
        || !text_has_completion_trigger(&state.text.borrow(), state.cursor.get())
    {
        dismiss_completion(state);
        return;
    }

    let request_id = next_completion_request_id(state);
    super::request_suggestions(
        state,
        request_id,
        state.cursor.get().min(state.text.borrow().len()),
    );
    position_completion_popover(area, state);
}

fn next_completion_request_id(state: &Rc<EditorState>) -> u64 {
    let mut completion = state.completion.borrow_mut();
    completion.request_id = completion.request_id.wrapping_add(1).max(1);
    completion.request_id
}

fn clear_completion(state: &Rc<EditorState>, invalidate_request: bool) {
    {
        let mut completion = state.completion.borrow_mut();
        if invalidate_request {
            completion.request_id = completion.request_id.wrapping_add(1).max(1);
        }
        completion.items.clear();
        completion.selected = 0;
        completion.replacement_range = None;
    }

    if let Some(ui) = state.completion_ui.borrow().as_ref() {
        ui.popover.popdown();
    }
}

fn show_completion_popover(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let ui = ensure_completion_ui(area, state);
    refill_completion_rows(&ui, state);
    select_completion_row(&ui, state.completion.borrow().selected);
    position_completion_popover(area, state);
    ui.popover.popup();
    area.grab_focus();
}

fn ensure_completion_ui(area: &gtk::DrawingArea, state: &Rc<EditorState>) -> CompletionUi {
    if let Some(ui) = state.completion_ui.borrow().as_ref().cloned() {
        return ui;
    }

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("code-editor-completion-list");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(220)
        .max_content_height(220)
        .propagate_natural_height(true)
        .child(&list)
        .build();

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(false)
        .position(gtk::PositionType::Bottom)
        .child(&scroller)
        .build();
    popover.add_css_class("menu");
    popover.add_css_class("code-editor-completion-popover");
    popover.set_halign(gtk::Align::Start);
    popover.set_parent(area);

    list.connect_row_activated({
        let area = area.clone();
        let state = state.clone();
        move |_, row| {
            let index = row.index();
            if index >= 0 {
                state.completion.borrow_mut().selected = index as usize;
            }
            accept_completion(&area, &state);
        }
    });

    let ui = CompletionUi { popover, list };
    state.completion_ui.replace(Some(ui.clone()));
    ui
}

fn refill_completion_rows(ui: &CompletionUi, state: &Rc<EditorState>) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }

    for item in state.completion.borrow().items.iter() {
        ui.list.append(&completion_row(&item.label));
    }
}

fn completion_row(label: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    label.add_css_class("code-editor-completion-label");

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    content.add_css_class("code-editor-completion-row");
    content.append(&label);

    gtk::ListBoxRow::builder()
        .selectable(true)
        .activatable(true)
        .child(&content)
        .build()
}

fn position_completion_popover(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let Some(ui) = state.completion_ui.borrow().as_ref().cloned() else {
        return;
    };
    let Some((x, y, width, height)) = render::cursor_rect(area, state) else {
        return;
    };

    ui.popover.set_pointing_to(Some(&gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        width.ceil().max(1.0) as i32,
        height.ceil().max(1.0) as i32,
    )));
}

fn select_completion_row(ui: &CompletionUi, selected: usize) {
    if let Some(row) = ui.list.row_at_index(selected as i32) {
        ui.list.select_row(Some(&row));
    }
}

fn accept_completion(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let Some((item, start, end)) = ({
        let completion = state.completion.borrow();
        completion.replacement_range.and_then(|(start, end)| {
            completion
                .items
                .get(completion.selected)
                .cloned()
                .map(|item| (item, start, end))
        })
    }) else {
        dismiss_completion(state);
        return;
    };

    commit_edit(
        area,
        state,
        start,
        end,
        &item.insert_text,
        start + item.insert_text.len(),
        None,
        true,
    );
    dismiss_completion(state);
}

fn text_has_completion_trigger(text: &str, cursor: usize) -> bool {
    let cursor = previous_char_boundary(text, cursor.min(text.len()));
    let prefix_start = identifier_start_before(text, cursor);
    let Some(dot) = previous_non_whitespace(text, prefix_start) else {
        return false;
    };
    text[dot..].starts_with('.')
}

fn identifier_start_before(text: &str, cursor: usize) -> usize {
    let mut start = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, start) {
        if !(ch == '_' || ch.is_ascii_alphanumeric()) {
            break;
        }
        start = previous;
    }
    start
}

fn previous_non_whitespace(text: &str, cursor: usize) -> Option<usize> {
    let mut offset = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, offset) {
        if !ch.is_whitespace() {
            return Some(previous);
        }
        offset = previous;
    }
    None
}

fn editor_context_menu_sections(
    state: &Rc<EditorState>,
    offset: usize,
) -> Vec<context_menu::ActionMenuSection<TextContextAction>> {
    let mut sections = Vec::new();
    if let Some(section) = spelling_correction_section(state, offset) {
        sections.push(section);
    }
    sections.extend(context_menu::text_context_menu_sections(
        TextContextMenuState {
            undo: MenuActionState::visible(action_enabled(state, TextContextAction::Undo)),
            redo: MenuActionState::visible(action_enabled(state, TextContextAction::Redo)),
            cut: MenuActionState::visible(action_enabled(state, TextContextAction::Cut)),
            copy: MenuActionState::visible(action_enabled(state, TextContextAction::Copy)),
            paste: MenuActionState::visible(action_enabled(state, TextContextAction::Paste)),
            select_all: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::SelectAll,
            )),
            fold_selection: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::FoldSelection,
            )),
            toggle_wrap: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::ToggleWrap,
            )),
            toggle_read_only: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::ToggleReadOnly,
            )),
        },
    ));
    sections
}

fn spelling_correction_section(
    state: &Rc<EditorState>,
    offset: usize,
) -> Option<context_menu::ActionMenuSection<TextContextAction>> {
    if !state.editable.get() || state.diff_rows.borrow().is_some() {
        return None;
    }
    let issue = spellcheck_issue_at(state, offset)?;
    let items = issue
        .corrections
        .iter()
        .take(5)
        .map(|correction| {
            context_menu::ActionMenuItem::new(
                format!("Replace with \"{correction}\""),
                TextContextAction::CorrectSpelling {
                    start: issue.start,
                    end: issue.end,
                    replacement: correction.clone(),
                },
                true,
            )
        })
        .collect::<Vec<_>>();
    (!items.is_empty()).then(|| context_menu::ActionMenuSection::new(items))
}

fn spellcheck_issue_at(state: &Rc<EditorState>, offset: usize) -> Option<SpellcheckIssue> {
    state
        .spellcheck_issues
        .borrow()
        .iter()
        .find(|issue| issue.start <= offset && offset <= issue.end)
        .cloned()
}

fn action_enabled(state: &Rc<EditorState>, action: TextContextAction) -> bool {
    let is_diff_document = state.diff_rows.borrow().is_some();
    match action {
        TextContextAction::CorrectSpelling { .. } => !is_diff_document && state.editable.get(),
        TextContextAction::Undo => {
            !is_diff_document && state.editable.get() && !state.undo_stack.borrow().is_empty()
        }
        TextContextAction::Redo => {
            !is_diff_document && state.editable.get() && !state.redo_stack.borrow().is_empty()
        }
        TextContextAction::Copy => selection_bounds(state).is_some(),
        TextContextAction::Cut => {
            !is_diff_document && state.editable.get() && selection_bounds(state).is_some()
        }
        TextContextAction::Paste => !is_diff_document && state.editable.get(),
        TextContextAction::SelectAll => !state.text.borrow().is_empty(),
        TextContextAction::FoldSelection => {
            !is_diff_document && state.editable.get() && selection_spans_lines(state)
        }
        TextContextAction::ToggleWrap => true,
        TextContextAction::ToggleReadOnly => !is_diff_document,
    }
}

fn run_action(area: &gtk::DrawingArea, state: &Rc<EditorState>, action: TextContextAction) {
    match action {
        TextContextAction::CorrectSpelling {
            start,
            end,
            replacement,
        } => {
            let cursor = start + replacement.len();
            commit_edit(area, state, start, end, &replacement, cursor, None, true);
        }
        TextContextAction::Undo => undo(area, state),
        TextContextAction::Redo => redo(area, state),
        TextContextAction::Copy => copy_selection(state),
        TextContextAction::Cut => {
            if state.editable.get() && selection_bounds(state).is_some() {
                copy_selection(state);
                delete_selection(area, state);
            }
        }
        TextContextAction::Paste => paste_from_clipboard(area, state),
        TextContextAction::SelectAll => {
            state.selection.replace(Some(Selection {
                anchor: 0,
                focus: state.text.borrow().len(),
                visual_anchor: 0,
                visual_focus: state.text.borrow().len(),
            }));
            state.cursor.set(state.text.borrow().len());
            area.queue_draw();
        }
        TextContextAction::ToggleWrap => {
            state.wrap.set(!state.wrap.get());
            render::invalidate_layout(state);
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
            area.queue_draw();
        }
        TextContextAction::ToggleReadOnly => {
            state.editable.set(!state.editable.get());
            area.set_focusable(true);
            area.set_cursor_from_name(None);
            state.cursor_visible.set(true);
            area.queue_draw();
        }
        TextContextAction::FoldSelection => fold_selection(area, state),
    }
}

fn copy_selection(state: &Rc<EditorState>) {
    let Some((start, end)) = selection_bounds(state) else {
        return;
    };
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let text = state.text.borrow();
    display.clipboard().set_text(&text[start..end]);
}

fn paste_from_clipboard(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let area = area.clone();
    let state = state.clone();
    gtk::glib::MainContext::default().spawn_local(async move {
        let Ok(Some(text)) = display.clipboard().read_text_future().await else {
            return;
        };
        if !state.editable.get() {
            return;
        }
        insert_text(&area, &state, &text);
    });
}

fn insert_text(area: &gtk::DrawingArea, state: &Rc<EditorState>, inserted: &str) {
    let text = state.text.borrow();
    let (start, end) = selection_bounds(state).unwrap_or_else(|| {
        let cursor = state.cursor.get().min(text.len());
        (cursor, cursor)
    });
    drop(text);
    let cursor = start + inserted.len();
    commit_edit(area, state, start, end, inserted, cursor, None, true);
}

fn insert_newline(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let text = state.text.borrow();
    let (start, end) = selection_bounds(state).unwrap_or_else(|| {
        let cursor = state.cursor.get().min(text.len());
        (cursor, cursor)
    });
    let language = state.language.borrow();
    let newline = enter_newline(NewlineContext {
        language: &language,
        text: &text,
        cursor: start,
    });
    drop(text);

    commit_edit(
        area,
        state,
        start,
        end,
        &newline.inserted,
        newline.cursor,
        None,
        true,
    );
}

#[derive(Clone, Copy)]
struct LinePrefixEdit {
    start: usize,
    removed: usize,
    inserted: usize,
}

fn edit_line_indentation(area: &gtk::DrawingArea, state: &Rc<EditorState>, indent: bool) {
    let text = state.text.borrow();
    let (range_start, range_end, replacement, edits) = line_indentation_edit(&text, state, indent);
    let before_cursor = state.cursor.get().min(text.len());
    let before_selection = *state.selection.borrow();
    drop(text);

    if edits.is_empty() {
        log::debug!(
            "code_editor indentation skipped action={}",
            if indent { "indent" } else { "outdent" }
        );
        return;
    }

    let cursor = map_offset_through_prefix_edits(before_cursor, &edits);
    let restored_selection = before_selection.map(|selection| Selection {
        anchor: map_offset_through_prefix_edits(selection.anchor, &edits),
        focus: map_offset_through_prefix_edits(selection.focus, &edits),
        visual_anchor: map_offset_through_prefix_edits(selection.visual_anchor, &edits),
        visual_focus: map_offset_through_prefix_edits(selection.visual_focus, &edits),
    });
    log::debug!(
        "code_editor indentation action={} range={}..{} lines={}",
        if indent { "indent" } else { "outdent" },
        range_start,
        range_end,
        edits.len()
    );
    commit_edit(
        area,
        state,
        range_start,
        range_end,
        &replacement,
        cursor,
        restored_selection,
        true,
    );
}

fn line_indentation_edit(
    text: &str,
    state: &Rc<EditorState>,
    indent: bool,
) -> (usize, usize, String, Vec<LinePrefixEdit>) {
    let (first_line_start, last_line_start) = indentation_line_range(text, state);
    let range_start = first_line_start;
    let range_end = current_line_end(text, last_line_start);
    let line_starts = line_starts_between(text, first_line_start, last_line_start);

    let mut replacement = String::with_capacity(
        range_end
            .saturating_sub(range_start)
            .saturating_add(line_starts.len() * INDENT_TEXT.len()),
    );
    let mut edits = Vec::new();
    let mut copied_until = range_start;

    for line_start in line_starts {
        replacement.push_str(&text[copied_until..line_start]);
        if indent {
            replacement.push_str(INDENT_TEXT);
            edits.push(LinePrefixEdit {
                start: line_start,
                removed: 0,
                inserted: INDENT_TEXT.len(),
            });
            copied_until = line_start;
            continue;
        }

        let line_end = current_line_end(text, line_start);
        let removed = outdent_len(&text[line_start..line_end]);
        if removed == 0 {
            copied_until = line_start;
            continue;
        }

        edits.push(LinePrefixEdit {
            start: line_start,
            removed,
            inserted: 0,
        });
        copied_until = line_start + removed;
    }

    replacement.push_str(&text[copied_until..range_end]);
    (range_start, range_end, replacement, edits)
}

fn indentation_line_range(text: &str, state: &Rc<EditorState>) -> (usize, usize) {
    let cursor = state.cursor.get().min(text.len());
    let (start, end) = selection_bounds(state).unwrap_or((cursor, cursor));
    let first_line_start = current_line_start(text, start);
    let last_offset = if end > start {
        previous_grapheme_offset(text, end)
    } else {
        start
    };
    let last_line_start = current_line_start(text, last_offset);
    (first_line_start, last_line_start)
}

fn line_starts_between(text: &str, first_line_start: usize, last_line_start: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut line_start = first_line_start.min(text.len());
    loop {
        starts.push(line_start);
        if line_start >= last_line_start {
            break;
        }

        let line_end = current_line_end(text, line_start);
        if line_end >= text.len() {
            break;
        }
        line_start = (line_end + 1).min(text.len());
    }
    starts
}

fn outdent_len(line: &str) -> usize {
    if line.starts_with('\t') {
        return '\t'.len_utf8();
    }

    line.as_bytes()
        .iter()
        .take(INDENT_TEXT.len())
        .take_while(|byte| **byte == b' ')
        .count()
}

fn map_offset_through_prefix_edits(offset: usize, edits: &[LinePrefixEdit]) -> usize {
    let mut adjustment = 0isize;

    for edit in edits {
        if offset < edit.start {
            break;
        }

        let adjusted_start = edit.start.saturating_add_signed(adjustment);
        let removed_end = edit.start.saturating_add(edit.removed);
        if offset <= removed_end {
            return adjusted_start.saturating_add(edit.inserted);
        }

        adjustment += edit.inserted as isize - edit.removed as isize;
    }

    offset.saturating_add_signed(adjustment)
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .take_while(|(_, ch)| matches!(ch, ' ' | '\t'))
        .map(|(offset, ch)| offset + ch.len_utf8())
        .last()
        .unwrap_or(0);
    &line[..end]
}

fn edit_backspace(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if delete_selection(area, state) {
        return;
    }
    let (start, cursor) = {
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        (previous_grapheme_offset(&text, cursor), cursor)
    };
    if start == cursor {
        return;
    }
    commit_edit(area, state, start, cursor, "", start, None, true);
}

fn edit_delete(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if delete_selection(area, state) {
        return;
    }
    let Some((cursor, end)) = ({
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        (cursor < text.len()).then(|| (cursor, next_grapheme_offset(&text, cursor)))
    }) else {
        return;
    };
    commit_edit(area, state, cursor, end, "", cursor, None, true);
}

fn edit_delete_leading_whitespace(area: &gtk::DrawingArea, state: &Rc<EditorState>) -> bool {
    if selection_bounds(state).is_some() {
        return false;
    }

    let Some((start, end)) = ({
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        let line_start = current_line_start(&text, cursor);
        let line_end = current_line_end(&text, line_start);
        let indent_end = line_start + leading_whitespace(&text[line_start..line_end]).len();
        (line_start < indent_end && cursor <= indent_end).then_some((line_start, indent_end))
    }) else {
        return false;
    };

    commit_edit(area, state, start, end, "", start, None, true);
    true
}

#[derive(Clone, Copy)]
enum DeleteDirection {
    Backward,
    Forward,
}

fn edit_delete_word(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    direction: DeleteDirection,
    extend_selection: bool,
) {
    if extend_selection {
        delete_word_range(area, state, direction);
        return;
    }

    if delete_selection(area, state) {
        return;
    }

    let (cursor, target) = {
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        let target = match direction {
            DeleteDirection::Backward => previous_word_start(&text, cursor),
            DeleteDirection::Forward => next_word_end(&text, cursor),
        };
        (cursor, target)
    };
    let (start, end) = ordered_offsets(cursor, target);
    if start == end {
        return;
    }

    commit_edit(area, state, start, end, "", start, None, true);
}

fn delete_word_range(area: &gtk::DrawingArea, state: &Rc<EditorState>, direction: DeleteDirection) {
    let text = state.text.borrow();
    let len = text.len();
    let selection = *state.selection.borrow();
    let anchor = selection
        .map(|selection| selection.anchor)
        .unwrap_or_else(|| state.cursor.get().min(len));
    let focus = selection
        .map(|selection| selection.focus)
        .unwrap_or_else(|| state.cursor.get().min(len));
    let target = match direction {
        DeleteDirection::Backward => previous_word_start(&text, focus),
        DeleteDirection::Forward => next_word_end(&text, focus),
    };
    let (start, end) = ordered_offsets(anchor, target);
    if start == end {
        drop(text);
        delete_selection(area, state);
        return;
    }

    drop(text);
    commit_edit(area, state, start, end, "", start, None, true);
}

fn delete_selection(area: &gtk::DrawingArea, state: &Rc<EditorState>) -> bool {
    let Some((start, end)) = selection_bounds(state) else {
        return false;
    };
    commit_edit(area, state, start, end, "", start, None, true);
    true
}

fn move_cursor_to(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    offset: usize,
    extend_selection: bool,
) {
    dismiss_completion(state);
    let text_len = state.text.borrow().len();
    let offset = offset.min(text_len);
    if extend_selection {
        let anchor = state
            .selection
            .borrow()
            .map(|selection| selection.anchor)
            .unwrap_or_else(|| state.cursor.get().min(text_len));
        state.selection.replace(Some(Selection {
            anchor,
            focus: offset,
            visual_anchor: anchor,
            visual_focus: offset,
        }));
    } else {
        state.selection.borrow_mut().take();
    }
    state.cursor.set(offset);
    render::ensure_offset_visible(area, state, offset);
    reset_cursor_blink(area, state);
}

fn move_cursor_vertical(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    delta: isize,
    extend_selection: bool,
) {
    let Some(target) = render::vertical_cursor_target(area, state, delta) else {
        if !extend_selection && state.selection.borrow().is_some() {
            move_cursor_to(area, state, state.cursor.get(), false);
        } else {
            dismiss_completion(state);
        }
        return;
    };

    move_cursor_to(area, state, target, extend_selection);
}

fn cursor_left_target(state: &Rc<EditorState>, by_word: bool, extend_selection: bool) -> usize {
    if !extend_selection {
        if let Some((start, _)) = selection_bounds(state) {
            return start;
        }
    }

    let text = state.text.borrow();
    let cursor = state.cursor.get().min(text.len());
    if by_word {
        previous_word_start(&text, cursor)
    } else {
        previous_grapheme_offset(&text, cursor)
    }
}

fn cursor_right_target(state: &Rc<EditorState>, by_word: bool, extend_selection: bool) -> usize {
    if !extend_selection {
        if let Some((_, end)) = selection_bounds(state) {
            return end;
        }
    }

    let text = state.text.borrow();
    let cursor = state.cursor.get().min(text.len());
    if by_word {
        next_word_start(&text, cursor)
    } else {
        next_grapheme_offset(&text, cursor)
    }
}

fn scroll_page(area: &gtk::DrawingArea, state: &Rc<EditorState>, direction: isize) {
    let line_height = render::line_height(state);
    let distance = (area.allocated_height() as f64 - line_height).max(line_height);
    render::set_scroll_y(
        area,
        state,
        state.scroll_y.get() + distance * direction as f64,
    );
}

fn page_line_delta(area: &gtk::DrawingArea, state: &Rc<EditorState>, direction: isize) -> isize {
    let line_height = render::line_height(state);
    let lines = ((area.allocated_height() as f64 - line_height).max(line_height) / line_height)
        .floor()
        .max(1.0) as isize;
    lines * direction
}

fn select_word_at(area: &gtk::DrawingArea, state: &Rc<EditorState>, offset: usize) -> bool {
    let bounds = {
        let text = state.text.borrow();
        word_bounds_at(&text, offset)
    };
    let Some((start, end)) = bounds else {
        return false;
    };

    let raw_anchor = offset.min(state.text.borrow().len());
    state.selection.replace(Some(Selection {
        anchor: raw_anchor,
        focus: raw_anchor,
        visual_anchor: start,
        visual_focus: end,
    }));
    state.cursor.set(end);
    render::ensure_offset_visible(area, state, end);
    reset_cursor_blink(area, state);
    true
}

fn select_line_at(area: &gtk::DrawingArea, state: &Rc<EditorState>, offset: usize) {
    let text = state.text.borrow();
    let (start, end) = logical_line_bounds_at(&text, offset);
    drop(text);

    state.selection.replace(Some(Selection {
        anchor: offset.min(state.text.borrow().len()),
        focus: offset.min(state.text.borrow().len()),
        visual_anchor: start,
        visual_focus: end,
    }));
    state.cursor.set(end);
    render::ensure_offset_visible(area, state, end);
    reset_cursor_blink(area, state);
}

fn selection_spans_lines(state: &Rc<EditorState>) -> bool {
    let Some((start, end)) = selection_bounds(state) else {
        return false;
    };
    let text = state.text.borrow();
    render::line_for_offset(&text, start) < render::line_for_offset(&text, end)
}

fn previous_word_start(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = previous;
    }

    let Some((_, ch)) = previous_char(text, offset) else {
        return offset;
    };
    let group = text_group(ch);
    while let Some((previous, ch)) = previous_char(text, offset) {
        if ch.is_whitespace() || text_group(ch) != group {
            break;
        }
        offset = previous;
    }

    offset
}

fn next_word_start(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    if let Some((_, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            let group = text_group(ch);
            while let Some((current, ch)) = next_char(text, offset) {
                if ch.is_whitespace() || text_group(ch) != group {
                    break;
                }
                offset = current + ch.len_utf8();
            }
        }
    }

    while let Some((current, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = current + ch.len_utf8();
    }

    offset
}

fn next_word_end(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    while let Some((current, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = current + ch.len_utf8();
    }

    let Some((_, ch)) = next_char(text, offset) else {
        return offset;
    };
    let group = text_group(ch);
    while let Some((current, ch)) = next_char(text, offset) {
        if ch.is_whitespace() || text_group(ch) != group {
            break;
        }
        offset = current + ch.len_utf8();
    }

    offset
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextGroup {
    Word,
    Punctuation,
}

fn text_group(ch: char) -> TextGroup {
    if ch == '_' || ch.is_alphanumeric() {
        TextGroup::Word
    } else {
        TextGroup::Punctuation
    }
}

fn previous_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    text[..cursor.min(text.len())].char_indices().last()
}

fn next_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .char_indices()
        .next()
        .map(|(offset, ch)| (cursor + offset, ch))
}

fn ordered_offsets(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

fn previous_grapheme_offset(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .grapheme_indices(true)
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

fn next_grapheme_offset(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .grapheme_indices(true)
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn current_line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .rfind('\n')
        .map(|offset| offset + 1)
        .unwrap_or(0)
}

fn smart_home_target(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    let line_start = current_line_start(text, cursor);
    let line_end = current_line_end(text, line_start);
    let indent_end = line_start + leading_whitespace(&text[line_start..line_end]).len();

    if indent_end > line_start && cursor != indent_end {
        indent_end
    } else {
        line_start
    }
}

fn current_line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn logical_line_bounds_at(text: &str, offset: usize) -> (usize, usize) {
    let start = current_line_start(text, offset);
    let line_end = current_line_end(text, offset);
    let end = if line_end < text.len() {
        next_grapheme_offset(text, line_end)
    } else {
        line_end
    };
    (start, end)
}

fn commit_edit(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    start: usize,
    old_end: usize,
    replacement: &str,
    cursor: usize,
    restored_selection: Option<Selection>,
    record_history: bool,
) {
    let before_cursor = state.cursor.get();
    let before_selection = *state.selection.borrow();
    let (start, old_end, removed, text_changed, folds_may_change) = {
        let text = state.text.borrow();
        let start = previous_char_boundary(&text, start.min(text.len()));
        let old_end = previous_char_boundary(&text, old_end.min(text.len()).max(start));
        let removed = text[start..old_end].to_string();
        let text_changed = removed != replacement;
        let folds_may_change = text_affects_folds(&removed) || text_affects_folds(replacement);
        (start, old_end, removed, text_changed, folds_may_change)
    };
    if text_changed && record_history {
        push_history_snapshot(
            &mut state.undo_stack.borrow_mut(),
            HistorySnapshot {
                start,
                removed: removed.clone(),
                inserted: replacement.to_string(),
                before_cursor,
                before_selection,
                after_cursor: cursor,
                after_selection: restored_selection,
            },
        );
        state.redo_stack.borrow_mut().clear();
    }

    if text_changed {
        state
            .text
            .borrow_mut()
            .replace_range(start, old_end, replacement);
        super::send_syntax_edit(
            state,
            start,
            old_end,
            replacement,
            folds_may_change
                && state.auto_folding_enabled.get()
                && state.diff_rows.borrow().is_none(),
        );
    }
    state.cursor.set(cursor);
    state.selection.replace(restored_selection);
    if text_changed {
        render::invalidate_layout(state);
        render::invalidate_highlights(state);
        super::clear_git_state_for_state(state);
    }
    super::normalize_folds_for_current_text(state, "edit");
    if text_changed {
        notify_edit(state);
    }
    render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
    render::ensure_offset_visible(area, state, cursor);
    reset_cursor_blink(area, state);
}

fn undo(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(snapshot) = state.undo_stack.borrow_mut().pop() else {
        return;
    };
    restore_snapshot(area, state, &snapshot, HistoryDirection::Undo);
    push_history_snapshot(&mut state.redo_stack.borrow_mut(), snapshot);
}

fn redo(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(snapshot) = state.redo_stack.borrow_mut().pop() else {
        return;
    };
    restore_snapshot(area, state, &snapshot, HistoryDirection::Redo);
    push_history_snapshot(&mut state.undo_stack.borrow_mut(), snapshot);
}

fn push_history_snapshot(stack: &mut Vec<HistorySnapshot>, snapshot: HistorySnapshot) {
    if stack.len() >= MAX_HISTORY_SNAPSHOTS {
        stack.remove(0);
    }
    stack.push(snapshot);
}

#[derive(Clone, Copy)]
enum HistoryDirection {
    Undo,
    Redo,
}

fn restore_snapshot(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    snapshot: &HistorySnapshot,
    direction: HistoryDirection,
) {
    match direction {
        HistoryDirection::Undo => commit_edit(
            area,
            state,
            snapshot.start,
            snapshot.start + snapshot.inserted.len(),
            &snapshot.removed,
            snapshot.before_cursor,
            snapshot.before_selection,
            false,
        ),
        HistoryDirection::Redo => commit_edit(
            area,
            state,
            snapshot.start,
            snapshot.start + snapshot.removed.len(),
            &snapshot.inserted,
            snapshot.after_cursor,
            snapshot.after_selection,
            false,
        ),
    }
}

fn previous_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn text_affects_folds(text: &str) -> bool {
    text.bytes()
        .any(|byte| matches!(byte, b'\n' | b'{' | b'}' | b'[' | b']' | b'(' | b')' | b':'))
}

fn fold_selection(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let Some((start, end)) = selection_bounds(state) else {
        return;
    };
    let text = state.text.borrow();
    let start_line = render::line_for_offset(&text, start);
    let end_line = render::line_for_offset(&text, end);
    if end_line <= start_line {
        return;
    }
    state.folds.borrow_mut().push(FoldRange {
        start_line,
        end_line,
        expanded: false,
        automatic: false,
    });
    if !super::normalize_folds_for_current_text(state, "selection fold") {
        super::mark_fold_state_changed(state);
    }
    state.selection.borrow_mut().take();
    render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
    area.queue_draw();
}

fn toggle_fold_at(area: &gtk::DrawingArea, state: &Rc<EditorState>, x: f64, y: f64) -> bool {
    let Some(action) = render::fold_action_at_point(area, state, x, y) else {
        return false;
    };
    match action {
        render::FoldAction::Toggle {
            index,
            start_line,
            end_line,
        } => {
            let toggled = {
                let mut folds = state.folds.borrow_mut();
                if let Some(fold) = folds
                    .get_mut(index)
                    .filter(|fold| fold.start_line == start_line && fold.end_line == end_line)
                {
                    fold.expanded = !fold.expanded;
                    true
                } else {
                    false
                }
            };
            if !toggled {
                log::debug!(
                    "code_editor fold toggle ignored stale index={index} range={start_line}..{end_line}"
                );
                return true;
            }
            super::mark_fold_state_changed(state);
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
            area.queue_draw();
        }
        render::FoldAction::Reveal(fold_index) => {
            notify_diff_fold(state, fold_index);
        }
    }
    true
}
