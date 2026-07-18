use gtk::glib::translate::{ToGlibPtr, from_glib};
use gtk::{gdk, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const MIDDLE_AUTOSCROLL_FRAME_MS: u64 = 16;
const MIDDLE_AUTOSCROLL_DEAD_ZONE: f64 = 12.0;
const MIDDLE_AUTOSCROLL_RAMP_DISTANCE: f64 = 150.0;
const MIDDLE_AUTOSCROLL_MIN_PIXELS_PER_FRAME: f64 = 8.0;
const MIDDLE_AUTOSCROLL_BASE_EXTRA_PIXELS_PER_FRAME: f64 = 120.0;
const MIDDLE_AUTOSCROLL_AGGRESSIVE_EXTRA_PIXELS_PER_FRAME: f64 = 120.0;
const MIDDLE_AUTOSCROLL_OUTSIDE_EXTRA_PIXELS_PER_FRAME: f64 = 180.0;
const MIDDLE_AUTOSCROLL_DIAGONAL_CURSOR_RATIO: f64 = 0.65;
const MIDDLE_AUTOSCROLL_DRAG_THRESHOLD: f64 = 4.0;
const MIDDLE_AUTOSCROLL_MARKER_RADIUS: f64 = 16.0;
const MIDDLE_AUTOSCROLL_MARKER_MARGIN: f64 = 3.0;

#[derive(Clone, Copy, Debug)]
struct MiddleAutoscrollPress {
    start_x: f64,
    start_y: f64,
    dragged: bool,
}

impl MiddleAutoscrollPress {
    fn new(start_x: f64, start_y: f64) -> Self {
        Self {
            start_x,
            start_y,
            dragged: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MiddleAutoscrollState {
    pub origin: Point,
    pub pointer: Point,
}

#[derive(Default)]
pub struct MiddleAutoscroll {
    state: Cell<Option<MiddleAutoscrollState>>,
    source: RefCell<Option<gtk::glib::SourceId>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoscrollAxes {
    Vertical,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AxisDirection {
    Negative,
    Neutral,
    Positive,
}

impl AxisDirection {
    fn for_offset(offset: f64) -> Self {
        if offset < -MIDDLE_AUTOSCROLL_DEAD_ZONE {
            Self::Negative
        } else if offset > MIDDLE_AUTOSCROLL_DEAD_ZONE {
            Self::Positive
        } else {
            Self::Neutral
        }
    }

    fn is_active(self) -> bool {
        self != Self::Neutral
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MarkerColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl MarkerColor {
    pub const fn rgba(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MarkerStyle {
    pub foreground: MarkerColor,
    pub background: MarkerColor,
    pub shadow: MarkerColor,
}

impl MiddleAutoscroll {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.state.get().is_some()
    }

    pub fn state(&self) -> Option<MiddleAutoscrollState> {
        self.state.get()
    }

    pub fn start(&self, x: f64, y: f64) -> MiddleAutoscrollState {
        let state = MiddleAutoscrollState {
            origin: Point::new(x, y),
            pointer: Point::new(x, y),
        };
        self.state.set(Some(state));
        state
    }

    pub fn update_pointer(&self, x: f64, y: f64) -> Option<MiddleAutoscrollState> {
        let mut state = self.state.get()?;
        state.pointer = Point::new(x, y);
        self.state.set(Some(state));
        Some(state)
    }

    pub fn stop(&self) -> bool {
        let was_active = self.state.get().is_some();
        self.state.set(None);
        if let Some(source) = self.source.borrow_mut().take() {
            source.remove();
        }
        was_active
    }

    pub fn finish_tick(&self) -> bool {
        let was_active = self.state.get().is_some();
        self.state.set(None);
        self.source.borrow_mut().take();
        was_active
    }

    pub fn has_source(&self) -> bool {
        self.source.borrow().is_some()
    }

    pub fn set_source(&self, source: gtk::glib::SourceId) {
        if let Some(previous) = self.source.borrow_mut().take() {
            previous.remove();
        }
        self.source.replace(Some(source));
    }
}

pub fn middle_autoscroll_frame_duration() -> Duration {
    Duration::from_millis(MIDDLE_AUTOSCROLL_FRAME_MS)
}

pub fn middle_autoscroll_delta(offset: f64) -> f64 {
    let distance = offset.abs();
    if distance <= MIDDLE_AUTOSCROLL_DEAD_ZONE {
        return 0.0;
    }

    let ratio =
        ((distance - MIDDLE_AUTOSCROLL_DEAD_ZONE) / MIDDLE_AUTOSCROLL_RAMP_DISTANCE).max(0.0);
    let ramp_ratio = ratio.min(1.0);
    let outside_ratio = (ratio - 1.0).max(0.0);
    let pixels_per_frame = MIDDLE_AUTOSCROLL_MIN_PIXELS_PER_FRAME
        + ramp_ratio.powi(2) * MIDDLE_AUTOSCROLL_BASE_EXTRA_PIXELS_PER_FRAME
        + ramp_ratio.powi(3) * MIDDLE_AUTOSCROLL_AGGRESSIVE_EXTRA_PIXELS_PER_FRAME
        + outside_ratio * MIDDLE_AUTOSCROLL_OUTSIDE_EXTRA_PIXELS_PER_FRAME;
    offset.signum() * pixels_per_frame
}

pub fn middle_autoscroll_cursor(
    state: MiddleAutoscrollState,
    axes: AutoscrollAxes,
) -> &'static str {
    let offset_x = state.pointer.x - state.origin.x;
    let offset_y = state.pointer.y - state.origin.y;
    let (horizontal, vertical) = cursor_directions(offset_x, offset_y, axes);

    match (horizontal, vertical) {
        (AxisDirection::Negative, AxisDirection::Negative) => "nw-resize",
        (AxisDirection::Positive, AxisDirection::Negative) => "ne-resize",
        (AxisDirection::Negative, AxisDirection::Positive) => "sw-resize",
        (AxisDirection::Positive, AxisDirection::Positive) => "se-resize",
        (AxisDirection::Negative, AxisDirection::Neutral) => "w-resize",
        (AxisDirection::Positive, AxisDirection::Neutral) => "e-resize",
        (AxisDirection::Neutral, AxisDirection::Negative) => "n-resize",
        (AxisDirection::Neutral, AxisDirection::Positive) => "s-resize",
        (AxisDirection::Neutral, AxisDirection::Neutral) => "all-scroll",
    }
}

type CanMiddleAutoscroll = Rc<dyn Fn() -> bool>;
type ApplyMiddleAutoscroll = Rc<dyn Fn(MiddleAutoscrollState)>;
type MiddleAutoscrollCallback = Rc<dyn Fn()>;
type SetMiddleAutoscrollCursor = Rc<dyn Fn(Option<&'static str>)>;

pub fn install_middle_autoscroll<
    W,
    CanScroll,
    ApplyScroll,
    PrepareStart,
    Cleanup,
    SetCursor,
    Redraw,
>(
    widget: &W,
    autoscroll: &Rc<MiddleAutoscroll>,
    axes: AutoscrollAxes,
    log_target: &'static str,
    can_scroll: CanScroll,
    apply_scroll: ApplyScroll,
    prepare_start: PrepareStart,
    cleanup: Cleanup,
    set_cursor: SetCursor,
    redraw: Redraw,
) where
    W: IsA<gtk::Widget> + 'static,
    CanScroll: Fn() -> bool + 'static,
    ApplyScroll: Fn(MiddleAutoscrollState) + 'static,
    PrepareStart: Fn() + 'static,
    Cleanup: Fn() + 'static,
    SetCursor: Fn(Option<&'static str>) + 'static,
    Redraw: Fn() + 'static,
{
    let widget = widget.as_ref().clone();
    let can_scroll: CanMiddleAutoscroll = Rc::new(can_scroll);
    let apply_scroll: ApplyMiddleAutoscroll = Rc::new(apply_scroll);
    let prepare_start: MiddleAutoscrollCallback = Rc::new(prepare_start);
    let cleanup: MiddleAutoscrollCallback = Rc::new(cleanup);
    let set_cursor: SetMiddleAutoscrollCursor = Rc::new(set_cursor);
    let redraw: MiddleAutoscrollCallback = Rc::new(redraw);
    let middle_press = Rc::new(Cell::new(None::<MiddleAutoscrollPress>));

    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let widget = widget.clone();
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let can_scroll = Rc::clone(&can_scroll);
        let apply_scroll = Rc::clone(&apply_scroll);
        let prepare_start = Rc::clone(&prepare_start);
        let cleanup = Rc::clone(&cleanup);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |gesture, _, x, y| {
            let button = gesture.current_button();
            if button == 2 {
                widget.grab_focus();
                if autoscroll.is_active() {
                    middle_press.set(None);
                    stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }

                if !(can_scroll)() {
                    middle_press.set(None);
                    return;
                }

                (prepare_start)();
                let state = autoscroll.start(x, y);
                middle_press.set(Some(MiddleAutoscrollPress::new(x, y)));
                log::debug!("{log_target} middle_autoscroll start x={x:.1} y={y:.1}");
                set_middle_autoscroll_cursor(&set_cursor, Some(state), axes);
                (redraw)();
                schedule_middle_autoscroll(
                    &widget,
                    &autoscroll,
                    axes,
                    &can_scroll,
                    &apply_scroll,
                    &cleanup,
                    &set_cursor,
                    &redraw,
                );
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }

            if autoscroll.is_active() {
                middle_press.set(None);
                stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    });
    click.connect_released({
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let cleanup = Rc::clone(&cleanup);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |gesture, _, x, y| {
            if gesture.current_button() != 2 && middle_button_is_down(gesture.current_event_state())
            {
                return;
            }
            let Some(press) = middle_press.take() else {
                return;
            };
            let dragged = press.dragged || middle_autoscroll_press_moved(press, x, y);
            update_middle_autoscroll_pointer(&autoscroll, x, y, axes, &set_cursor, &redraw);
            if dragged {
                log::debug!("{log_target} middle_autoscroll drag_release x={x:.1} y={y:.1}");
                stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    widget.add_controller(click);

    let middle_drag = gtk::GestureDrag::new();
    middle_drag.set_button(2);
    middle_drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    middle_drag.connect_drag_update({
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |gesture, offset_x, offset_y| {
            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            let pointer_x = start_x + offset_x;
            let pointer_y = start_y + offset_y;
            mark_middle_autoscroll_drag(&middle_press, pointer_x, pointer_y, log_target);
            update_middle_autoscroll_pointer(
                &autoscroll,
                pointer_x,
                pointer_y,
                axes,
                &set_cursor,
                &redraw,
            );
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    middle_drag.connect_drag_end({
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let cleanup = Rc::clone(&cleanup);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |_, offset_x, offset_y| {
            let Some(press) = middle_press.take() else {
                return;
            };
            let pointer_x = press.start_x + offset_x;
            let pointer_y = press.start_y + offset_y;
            update_middle_autoscroll_pointer(
                &autoscroll,
                pointer_x,
                pointer_y,
                axes,
                &set_cursor,
                &redraw,
            );
            if press.dragged {
                log::debug!(
                    "{log_target} middle_autoscroll drag_end x={pointer_x:.1} y={pointer_y:.1}"
                );
                stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
            } else {
                middle_press.set(Some(press));
            }
        }
    });
    widget.add_controller(middle_drag);

    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    motion.connect_motion({
        let autoscroll = Rc::clone(autoscroll);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        let middle_press = Rc::clone(&middle_press);

        move |controller, x, y| {
            if middle_button_is_down(controller.current_event_state()) {
                mark_middle_autoscroll_drag(&middle_press, x, y, log_target);
            }
            update_middle_autoscroll_pointer(&autoscroll, x, y, axes, &set_cursor, &redraw);
        }
    });
    motion.connect_leave({
        let widget = widget.clone();
        let autoscroll = Rc::clone(autoscroll);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |controller| {
            if !autoscroll.is_active() {
                return;
            }
            if let Some((x, y)) = controller
                .current_event()
                .and_then(|event| event.position())
            {
                let pointer = middle_autoscroll_edge_pointer(&widget, x, y);
                update_middle_autoscroll_pointer(
                    &autoscroll,
                    pointer.x,
                    pointer.y,
                    axes,
                    &set_cursor,
                    &redraw,
                );
            }
        }
    });
    widget.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let cleanup = Rc::clone(&cleanup);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |_, key, _, _| {
            if key == gdk::Key::Escape && autoscroll.is_active() {
                middle_press.set(None);
                stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
                return gtk::glib::Propagation::Stop;
            }

            gtk::glib::Propagation::Proceed
        }
    });
    widget.add_controller(keys);

    widget.connect_has_focus_notify({
        let autoscroll = Rc::clone(autoscroll);
        let middle_press = Rc::clone(&middle_press);
        let cleanup = Rc::clone(&cleanup);
        let set_cursor = Rc::clone(&set_cursor);
        let redraw = Rc::clone(&redraw);

        move |widget| {
            if !widget.has_focus() {
                middle_press.set(None);
                stop_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
            }
        }
    });
}

pub fn install_scrolled_window_middle_autoscroll(
    scroller: &gtk::ScrolledWindow,
    marker: &gtk::DrawingArea,
    axes: AutoscrollAxes,
    log_target: &'static str,
) -> Rc<MiddleAutoscroll> {
    let autoscroll = Rc::new(MiddleAutoscroll::new());
    install_scrolled_window_middle_autoscroll_with_state(
        scroller,
        marker,
        &autoscroll,
        axes,
        log_target,
        {
            let scroller = scroller.clone();
            move |cursor| scroller.set_cursor_from_name(cursor)
        },
    );
    autoscroll
}

pub fn install_scrolled_window_middle_autoscroll_with_state<SetCursor>(
    scroller: &gtk::ScrolledWindow,
    marker: &gtk::DrawingArea,
    autoscroll: &Rc<MiddleAutoscroll>,
    axes: AutoscrollAxes,
    log_target: &'static str,
    set_cursor: SetCursor,
) where
    SetCursor: Fn(Option<&'static str>) + 'static,
{
    marker.set_can_target(false);
    scroller.set_focusable(true);
    install_middle_autoscroll_marker(marker, autoscroll, axes);
    install_middle_autoscroll(
        scroller,
        autoscroll,
        axes,
        log_target,
        {
            let scroller = scroller.clone();
            move || scrolled_window_autoscroll_has_axis(&scroller, axes)
        },
        {
            let scroller = scroller.clone();
            move |state| apply_scrolled_window_middle_autoscroll(&scroller, axes, state)
        },
        || {},
        || {},
        set_cursor,
        {
            let marker = marker.clone();
            move || marker.queue_draw()
        },
    );
}

pub fn install_middle_autoscroll_marker(
    marker: &gtk::DrawingArea,
    autoscroll: &Rc<MiddleAutoscroll>,
    axes: AutoscrollAxes,
) {
    let autoscroll = Rc::clone(autoscroll);
    marker.set_draw_func(move |_, context, width, height| {
        draw_middle_autoscroll_marker(
            context,
            width,
            height,
            autoscroll.state(),
            axes,
            MarkerStyle {
                foreground: MarkerColor::rgba(0.95, 0.95, 0.95, 0.88),
                background: MarkerColor::rgba(0.08, 0.09, 0.1, 0.9),
                shadow: MarkerColor::rgba(0.0, 0.0, 0.0, 0.38),
            },
        );
    });
}

fn update_middle_autoscroll_pointer(
    autoscroll: &MiddleAutoscroll,
    x: f64,
    y: f64,
    axes: AutoscrollAxes,
    set_cursor: &SetMiddleAutoscrollCursor,
    redraw: &MiddleAutoscrollCallback,
) {
    if let Some(state) = autoscroll.update_pointer(x, y) {
        set_middle_autoscroll_cursor(set_cursor, Some(state), axes);
        (redraw)();
    }
}

fn schedule_middle_autoscroll(
    widget: &gtk::Widget,
    autoscroll: &Rc<MiddleAutoscroll>,
    axes: AutoscrollAxes,
    can_scroll: &CanMiddleAutoscroll,
    apply_scroll: &ApplyMiddleAutoscroll,
    cleanup: &MiddleAutoscrollCallback,
    set_cursor: &SetMiddleAutoscrollCursor,
    redraw: &MiddleAutoscrollCallback,
) {
    if autoscroll.has_source() {
        return;
    }

    let autoscroll_for_source = Rc::clone(autoscroll);
    let widget = widget.clone();
    let autoscroll = Rc::clone(autoscroll);
    let can_scroll = Rc::clone(can_scroll);
    let apply_scroll = Rc::clone(apply_scroll);
    let cleanup = Rc::clone(cleanup);
    let set_cursor = Rc::clone(set_cursor);
    let redraw = Rc::clone(redraw);
    let source = gtk::glib::timeout_add_local(middle_autoscroll_frame_duration(), move || {
        let Some(mut state) = autoscroll.state() else {
            finish_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
            return gtk::glib::ControlFlow::Break;
        };

        if !widget.is_visible() || !(can_scroll)() {
            finish_middle_autoscroll_session(&autoscroll, &cleanup, &set_cursor, &redraw);
            return gtk::glib::ControlFlow::Break;
        }

        if let Some(pointer) = middle_autoscroll_surface_pointer(&widget) {
            state = autoscroll
                .update_pointer(pointer.x, pointer.y)
                .unwrap_or(state);
            set_middle_autoscroll_cursor(&set_cursor, Some(state), axes);
            (redraw)();
        }

        (apply_scroll)(state);

        gtk::glib::ControlFlow::Continue
    });
    autoscroll_for_source.set_source(source);
}

fn stop_middle_autoscroll_session(
    autoscroll: &MiddleAutoscroll,
    cleanup: &MiddleAutoscrollCallback,
    set_cursor: &SetMiddleAutoscrollCursor,
    redraw: &MiddleAutoscrollCallback,
) {
    if autoscroll.stop() {
        (cleanup)();
        set_cursor(None);
        (redraw)();
    }
}

fn finish_middle_autoscroll_session(
    autoscroll: &MiddleAutoscroll,
    cleanup: &MiddleAutoscrollCallback,
    set_cursor: &SetMiddleAutoscrollCursor,
    redraw: &MiddleAutoscrollCallback,
) {
    if autoscroll.finish_tick() {
        (cleanup)();
        set_cursor(None);
        (redraw)();
    }
}

fn set_middle_autoscroll_cursor(
    set_cursor: &SetMiddleAutoscrollCursor,
    state: Option<MiddleAutoscrollState>,
    axes: AutoscrollAxes,
) {
    set_cursor(state.map(|state| middle_autoscroll_cursor(state, axes)));
}

fn apply_scrolled_window_middle_autoscroll(
    scroller: &gtk::ScrolledWindow,
    axes: AutoscrollAxes,
    state: MiddleAutoscrollState,
) {
    if axes == AutoscrollAxes::Both {
        let horizontal_delta = middle_autoscroll_delta(state.pointer.x - state.origin.x);
        if horizontal_delta.abs() > f64::EPSILON {
            let hadjustment = scroller.hadjustment();
            set_adjustment_value_clamped(&hadjustment, hadjustment.value() + horizontal_delta);
        }
    }

    let vertical_delta = middle_autoscroll_delta(state.pointer.y - state.origin.y);
    if vertical_delta.abs() > f64::EPSILON {
        let vadjustment = scroller.vadjustment();
        set_adjustment_value_clamped(&vadjustment, vadjustment.value() + vertical_delta);
    }
}

fn scrolled_window_autoscroll_has_axis(
    scroller: &gtk::ScrolledWindow,
    axes: AutoscrollAxes,
) -> bool {
    let vadjustment = scroller.vadjustment();
    adjustment_is_scrollable(&vadjustment)
        || (axes == AutoscrollAxes::Both && {
            let hadjustment = scroller.hadjustment();
            adjustment_is_scrollable(&hadjustment)
        })
}

fn adjustment_is_scrollable(adjustment: &gtk::Adjustment) -> bool {
    adjustment.upper() - adjustment.page_size() > adjustment.lower() + f64::EPSILON
}

fn set_adjustment_value_clamped(adjustment: &gtk::Adjustment, value: f64) {
    let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value(value.clamp(adjustment.lower(), max));
}

fn mark_middle_autoscroll_drag(
    middle_press: &Rc<Cell<Option<MiddleAutoscrollPress>>>,
    pointer_x: f64,
    pointer_y: f64,
    log_target: &'static str,
) {
    let Some(mut press) = middle_press.get() else {
        return;
    };
    if press.dragged {
        return;
    }

    let dx = pointer_x - press.start_x;
    let dy = pointer_y - press.start_y;
    if dx.hypot(dy) < MIDDLE_AUTOSCROLL_DRAG_THRESHOLD {
        return;
    }

    press.dragged = true;
    middle_press.set(Some(press));
    log::debug!("{log_target} middle_autoscroll drag_begin dx={dx:.1} dy={dy:.1}");
}

fn middle_autoscroll_press_moved(
    press: MiddleAutoscrollPress,
    pointer_x: f64,
    pointer_y: f64,
) -> bool {
    let dx = pointer_x - press.start_x;
    let dy = pointer_y - press.start_y;
    dx.hypot(dy) >= MIDDLE_AUTOSCROLL_DRAG_THRESHOLD
}

fn middle_button_is_down(modifiers: gdk::ModifierType) -> bool {
    modifiers.contains(gdk::ModifierType::BUTTON2_MASK)
}

fn cursor_directions(
    offset_x: f64,
    offset_y: f64,
    axes: AutoscrollAxes,
) -> (AxisDirection, AxisDirection) {
    let mut horizontal = match axes {
        AutoscrollAxes::Both => AxisDirection::for_offset(offset_x),
        AutoscrollAxes::Vertical => AxisDirection::Neutral,
    };
    let mut vertical = AxisDirection::for_offset(offset_y);

    if horizontal.is_active()
        && vertical.is_active()
        && !is_diagonal_cursor_offset(offset_x, offset_y)
    {
        if offset_x.abs() > offset_y.abs() {
            vertical = AxisDirection::Neutral;
        } else {
            horizontal = AxisDirection::Neutral;
        }
    }

    (horizontal, vertical)
}

fn is_diagonal_cursor_offset(offset_x: f64, offset_y: f64) -> bool {
    let x_distance = offset_x.abs();
    let y_distance = offset_y.abs();
    let diagonal_ratio = x_distance.min(y_distance) / x_distance.max(y_distance);
    diagonal_ratio >= MIDDLE_AUTOSCROLL_DIAGONAL_CURSOR_RATIO
}

pub fn middle_autoscroll_edge_pointer(_widget: &impl IsA<gtk::Widget>, x: f64, y: f64) -> Point {
    Point::new(x, y)
}

pub fn middle_autoscroll_surface_pointer(widget: &impl IsA<gtk::Widget>) -> Option<Point> {
    let display = gdk::Display::default()?;
    let pointer = display.default_seat()?.pointer()?;
    let (pointer_surface, surface_x, surface_y) = pointer.surface_at_position();
    let pointer_surface = pointer_surface?;
    pointer_surface_position_to_widget(widget, &pointer_surface, surface_x, surface_y)
}

fn pointer_surface_position_to_widget(
    widget: &impl IsA<gtk::Widget>,
    pointer_surface: &gdk::Surface,
    surface_x: f64,
    surface_y: f64,
) -> Option<Point> {
    let widget = widget.as_ref();
    let native = widget.native()?;
    let native_surface = native.surface()?;
    let (mut native_x, mut native_y) = (surface_x, surface_y);
    if pointer_surface != &native_surface
        && !translate_surface_coordinates(
            pointer_surface,
            &native_surface,
            &mut native_x,
            &mut native_y,
        )
    {
        return None;
    }

    let (surface_origin_x, surface_origin_y) = native.surface_transform();
    let root = widget.root()?.upcast::<gtk::Widget>();
    let (x, y) = root.translate_coordinates(
        widget,
        native_x - surface_origin_x,
        native_y - surface_origin_y,
    )?;
    Some(Point::new(x, y))
}

fn translate_surface_coordinates(
    from: &gdk::Surface,
    to: &gdk::Surface,
    x: &mut f64,
    y: &mut f64,
) -> bool {
    unsafe {
        from_glib(gdk::ffi::gdk_surface_translate_coordinates(
            from.to_glib_none().0,
            to.to_glib_none().0,
            x as *mut f64,
            y as *mut f64,
        ))
    }
}

pub fn draw_middle_autoscroll_marker(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    state: Option<MiddleAutoscrollState>,
    axes: AutoscrollAxes,
    style: MarkerStyle,
) {
    let Some(state) = state else {
        return;
    };

    let radius = MIDDLE_AUTOSCROLL_MARKER_RADIUS;
    let margin = radius + MIDDLE_AUTOSCROLL_MARKER_MARGIN;
    let x = state
        .origin
        .x
        .clamp(margin, (width as f64 - margin).max(margin));
    let y = state
        .origin
        .y
        .clamp(margin, (height as f64 - margin).max(margin));

    let _ = context.save();
    context.arc(x + 1.0, y + 1.0, radius, 0.0, std::f64::consts::PI * 2.0);
    set_source(context, style.shadow);
    let _ = context.fill();

    context.arc(x, y, radius, 0.0, std::f64::consts::PI * 2.0);
    set_source(context, style.background);
    let _ = context.fill_preserve();
    set_source(context, style.foreground);
    context.set_line_width(1.1);
    let _ = context.stroke();

    context.arc(x, y, 2.2, 0.0, std::f64::consts::PI * 2.0);
    let _ = context.fill();

    draw_vertical_marker_arrows(context, x, y);
    if axes == AutoscrollAxes::Both {
        draw_horizontal_marker_arrows(context, x, y);
    }
    let _ = context.restore();
}

fn set_source(context: &gtk::cairo::Context, color: MarkerColor) {
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
}

fn draw_vertical_marker_arrows(context: &gtk::cairo::Context, x: f64, y: f64) {
    context.move_to(x, y - 11.0);
    context.line_to(x - 4.5, y - 5.0);
    context.line_to(x + 4.5, y - 5.0);
    context.close_path();
    let _ = context.fill();

    context.move_to(x, y + 11.0);
    context.line_to(x - 4.5, y + 5.0);
    context.line_to(x + 4.5, y + 5.0);
    context.close_path();
    let _ = context.fill();
}

fn draw_horizontal_marker_arrows(context: &gtk::cairo::Context, x: f64, y: f64) {
    context.move_to(x - 11.0, y);
    context.line_to(x - 5.0, y - 4.5);
    context.line_to(x - 5.0, y + 4.5);
    context.close_path();
    let _ = context.fill();

    context.move_to(x + 11.0, y);
    context.line_to(x + 5.0, y - 4.5);
    context.line_to(x + 5.0, y + 4.5);
    context.close_path();
    let _ = context.fill();
}
