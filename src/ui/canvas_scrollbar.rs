use adw::prelude::*;
use gtk::cairo;
use gtk::gdk::RGBA;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub(in crate::ui) const WIDTH: f64 = 24.0;
pub(in crate::ui) const MIN_THUMB: f64 = 40.0;

const IDLE_LANE_WIDTH: f64 = 11.0;
const HOVER_LANE_WIDTH: f64 = 24.0;
const IDLE_TROUGH_MARGIN: f64 = 4.0;
const HOVER_TROUGH_MARGIN: f64 = 8.0;
const TROUGH_VERTICAL_MARGIN: f64 = 9.0;
const HOVER_ANIMATION_DURATION_MS: f64 = 200.0;
const HOVER_ANIMATION_FRAME_MS: f64 = 16.0;
const HOVER_TROUGH_ALPHA: f64 = 0.10;
const IDLE_THUMB_ALPHA: f64 = 0.14;
const HOVER_THUMB_ALPHA: f64 = 0.24;
const ACTIVE_THUMB_ALPHA: f64 = 0.38;
const IDLE_OUTLINE_ALPHA: f64 = 0.35;
const HOVER_OUTLINE_ALPHA: f64 = 0.60;
const HANDLE_OUTLINE_WIDTH: f64 = 1.0;
const SCROLL_ANIMATION_DURATION_MS: u32 = 200;

#[derive(Clone, Copy)]
pub(in crate::ui) enum MarkerKind {
    Added,
    Deleted,
    Mixed,
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct Theme {
    foreground: Color,
    outline: Color,
}

impl Theme {
    pub(in crate::ui) fn for_widget(widget: &impl IsA<gtk::Widget>) -> Self {
        let style_manager = adw::StyleManager::for_display(&widget.display());
        let outline = if style_manager.is_dark() {
            Color::rgba(0.0, 0.0, 12.0 / 255.0, 0.95)
        } else {
            Color::rgba(1.0, 1.0, 1.0, 1.0)
        };

        Self {
            foreground: Color::from_rgba(widget.color()),
            outline,
        }
    }
}

#[derive(Clone, Copy)]
struct Color {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

impl Color {
    const fn rgba(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    fn from_rgba(rgba: RGBA) -> Self {
        Self {
            red: rgba.red() as f64,
            green: rgba.green() as f64,
            blue: rgba.blue() as f64,
            alpha: rgba.alpha() as f64,
        }
    }

    fn with_alpha(self, alpha: f64) -> Self {
        Self {
            alpha: self.alpha * alpha.clamp(0.0, 1.0),
            ..self
        }
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn tuple(self) -> (f64, f64, f64, f64) {
        (self.x, self.y, self.width, self.height)
    }

    fn radius(self) -> f64 {
        self.width / 2.0
    }

    fn outset(self, amount: f64) -> Self {
        Self {
            x: self.x - amount,
            y: self.y - amount,
            width: self.width + amount * 2.0,
            height: self.height + amount * 2.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct Drag {
    start_scroll_y: f64,
}

impl Drag {
    pub(in crate::ui) fn new(start_scroll_y: f64) -> Self {
        Self { start_scroll_y }
    }

    pub(in crate::ui) fn scroll_for_delta(
        self,
        delta_y: f64,
        viewport_height: f64,
        thumb_height: f64,
        max_scroll: f64,
    ) -> f64 {
        let track_height = (viewport_height - (TROUGH_VERTICAL_MARGIN * 2.0)).max(1.0);
        let travel = (track_height - thumb_height).max(1.0);
        self.start_scroll_y + (delta_y / travel) * max_scroll
    }
}

pub(in crate::ui) struct SmoothScroll {
    animation: RefCell<Option<adw::TimedAnimation>>,
    target_value: Cell<f64>,
}

impl SmoothScroll {
    pub(in crate::ui) fn new() -> Self {
        Self {
            animation: RefCell::new(None),
            target_value: Cell::new(0.0),
        }
    }

    pub(in crate::ui) fn pause(&self) {
        if let Some(animation) = self.animation.borrow().as_ref() {
            animation.pause();
        }
    }

    pub(in crate::ui) fn scroll_relative<Apply>(
        &self,
        widget: &impl IsA<gtk::Widget>,
        current_value: f64,
        delta: f64,
        lower: f64,
        upper: f64,
        apply: Apply,
    ) -> bool
    where
        Apply: Fn(f64) + 'static,
    {
        if delta.abs() <= f64::EPSILON || upper <= lower {
            return false;
        }

        let animation = self.animation(widget, apply);
        let base = if animation.state() == adw::AnimationState::Playing {
            self.target_value.get()
        } else {
            current_value
        };
        let target = (base + delta).clamp(lower, upper);
        if (target - current_value).abs() <= f64::EPSILON {
            return false;
        }

        self.target_value.set(target);
        animation.set_value_from(current_value);
        animation.set_value_to(target);
        animation.set_duration(SCROLL_ANIMATION_DURATION_MS);
        animation.play();
        true
    }

    fn animation<Apply>(&self, widget: &impl IsA<gtk::Widget>, apply: Apply) -> adw::TimedAnimation
    where
        Apply: Fn(f64) + 'static,
    {
        if let Some(animation) = self.animation.borrow().as_ref() {
            return animation.clone();
        }

        let target = adw::CallbackAnimationTarget::new(apply);
        let animation =
            adw::TimedAnimation::new(widget, 0.0, 0.0, SCROLL_ANIMATION_DURATION_MS, target);
        animation.set_easing(adw::Easing::Ease);
        self.animation.replace(Some(animation.clone()));
        animation
    }
}

pub(in crate::ui) fn content_width(width: i32) -> i32 {
    (width - WIDTH.ceil() as i32).max(1)
}

pub(in crate::ui) fn max_scroll(total_height: f64, viewport_height: f64) -> f64 {
    (total_height - viewport_height).max(0.0)
}

pub(in crate::ui) fn mouse_wheel_delta(page_size: f64, dy: f64) -> f64 {
    let page_size = page_size.max(1.0);
    let pow_unit = page_size.powf(2.0 / 3.0);
    dy * pow_unit.min(page_size / 2.0)
}

pub(in crate::ui) fn is_mouse_scroll(controller: &gtk::EventControllerScroll) -> bool {
    controller
        .current_event_device()
        .is_some_and(|device| device.source() == gtk::gdk::InputSource::Mouse)
}

pub(in crate::ui) fn point_in_lane(width: i32, height: i32, total_height: f64, x: f64) -> bool {
    x >= width as f64 - WIDTH
        && x <= width as f64
        && thumb_rect(width, height, total_height, 0.0).is_some()
}

pub(in crate::ui) fn thumb_rect(
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
) -> Option<(f64, f64, f64, f64)> {
    let viewport_height = height.max(1) as f64;
    let total_height = total_height.max(viewport_height);
    if total_height <= viewport_height + 0.5 {
        return None;
    }

    let track = track_rect(width, height);
    let thumb_height = (track.height * viewport_height / total_height)
        .max(MIN_THUMB)
        .min(track.height);
    let max_scroll = max_scroll(total_height, viewport_height).max(1.0);
    let travel = (track.height - thumb_height).max(0.0);
    let y = track.y + (scroll_y.clamp(0.0, max_scroll) / max_scroll) * travel;

    Some((track.x, y, track.width, thumb_height))
}

pub(in crate::ui) fn scroll_for_lane_press(
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
    x: f64,
    y: f64,
) -> Option<f64> {
    if !point_in_lane(width, height, total_height, x) {
        return None;
    }

    let viewport_height = height.max(1) as f64;
    let track = track_rect(width, height);
    let (_, thumb_y, _, thumb_height) = thumb_rect(width, height, total_height, scroll_y)?;
    if y >= thumb_y && y <= thumb_y + thumb_height {
        return Some(scroll_y);
    }

    let travel = (track.height - thumb_height).max(0.0);
    let max_scroll = max_scroll(total_height, viewport_height);

    if travel <= f64::EPSILON || max_scroll <= f64::EPSILON {
        return Some(0.0);
    }

    let thumb_y = (y - (thumb_height / 2.0)).clamp(track.y, track.y + travel);
    Some(((thumb_y - track.y) / travel) * max_scroll)
}

fn track_rect(width: i32, height: i32) -> Rect {
    Rect {
        x: width as f64 - WIDTH,
        y: TROUGH_VERTICAL_MARGIN,
        width: WIDTH,
        height: (height as f64 - (TROUGH_VERTICAL_MARGIN * 2.0)).max(1.0),
    }
}

pub(in crate::ui) fn visual_track_rect(
    width: i32,
    height: i32,
    hover_progress: f64,
) -> (f64, f64, f64, f64) {
    handle_rect(width, height, hover_progress).tuple()
}

fn handle_rect(width: i32, height: i32, hover_progress: f64) -> Rect {
    let track = track_rect(width, height);
    let progress = hover_progress.clamp(0.0, 1.0);
    let lane_width = lerp(IDLE_LANE_WIDTH, HOVER_LANE_WIDTH, progress);
    let margin = lerp(IDLE_TROUGH_MARGIN, HOVER_TROUGH_MARGIN, progress);

    Rect {
        x: width as f64 - lane_width + margin,
        y: track.y,
        width: (lane_width - margin * 2.0).max(1.0),
        height: track.height,
    }
}

pub(in crate::ui) fn draw_track(
    context: &cairo::Context,
    width: i32,
    height: i32,
    total_height: f64,
    hover_progress: f64,
    theme: Theme,
) {
    if thumb_rect(width, height, total_height, 0.0).is_none() {
        return;
    }

    let handle = handle_rect(width, height, hover_progress);
    let alpha = HOVER_TROUGH_ALPHA * hover_progress.clamp(0.0, 1.0);
    if alpha <= f64::EPSILON {
        return;
    }

    fill_rounded_rect(
        context,
        handle.x,
        handle.y,
        handle.width,
        handle.height,
        handle.radius(),
        theme.foreground.with_alpha(alpha),
    );
}

pub(in crate::ui) fn clip_to_track(
    context: &cairo::Context,
    width: i32,
    height: i32,
    hover_progress: f64,
) {
    let handle = handle_rect(width, height, hover_progress);
    rounded_rect(
        context,
        handle.x,
        handle.y,
        handle.width,
        handle.height,
        handle.radius(),
    );
    context.clip();
}

pub(in crate::ui) fn draw_marker(
    context: &cairo::Context,
    kind: MarkerKind,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    hover_progress: f64,
) {
    let added = lerp_color((0.12, 0.46, 0.22), (0.18, 0.58, 0.28), hover_progress);
    let deleted = lerp_color((0.54, 0.13, 0.15), (0.68, 0.18, 0.20), hover_progress);
    let alpha = lerp(0.36, 0.50, hover_progress);

    match kind {
        MarkerKind::Added => fill_rect_rgba(context, x, y, width, height, added, alpha),
        MarkerKind::Deleted => fill_rect_rgba(context, x, y, width, height, deleted, alpha),
        MarkerKind::Mixed => {
            fill_rect_rgba(context, x, y, width / 2.0, height, deleted, alpha);
            fill_rect_rgba(
                context,
                x + width / 2.0,
                y,
                width / 2.0,
                height,
                added,
                alpha,
            );
        }
    }
}

pub(in crate::ui) fn draw_thumb(
    context: &cairo::Context,
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
    hover_progress: f64,
    active: bool,
    theme: Theme,
) {
    draw_thumb_outline(
        context,
        width,
        height,
        total_height,
        scroll_y,
        hover_progress,
        active,
        theme,
    );
    draw_thumb_fill(
        context,
        width,
        height,
        total_height,
        scroll_y,
        hover_progress,
        active,
        theme,
    );
}

pub(in crate::ui) fn draw_thumb_fill(
    context: &cairo::Context,
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
    hover_progress: f64,
    active: bool,
    theme: Theme,
) {
    let Some(thumb) = handle_thumb_rect(width, height, total_height, scroll_y, hover_progress)
    else {
        return;
    };

    let thumb_alpha = if active {
        ACTIVE_THUMB_ALPHA
    } else {
        lerp(IDLE_THUMB_ALPHA, HOVER_THUMB_ALPHA, hover_progress)
    };

    fill_rounded_rect(
        context,
        thumb.x,
        thumb.y,
        thumb.width,
        thumb.height,
        thumb.radius(),
        theme.foreground.with_alpha(thumb_alpha),
    );
}

pub(in crate::ui) fn draw_thumb_outline(
    context: &cairo::Context,
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
    hover_progress: f64,
    active: bool,
    theme: Theme,
) {
    let Some(thumb) = handle_thumb_rect(width, height, total_height, scroll_y, hover_progress)
    else {
        return;
    };

    let outline_alpha = if active {
        HOVER_OUTLINE_ALPHA
    } else {
        lerp(IDLE_OUTLINE_ALPHA, HOVER_OUTLINE_ALPHA, hover_progress)
    };

    let outline = thumb.outset(HANDLE_OUTLINE_WIDTH);
    fill_rounded_rect(
        context,
        outline.x,
        outline.y,
        outline.width,
        outline.height,
        outline.radius(),
        theme.outline.with_alpha(outline_alpha),
    );
}

fn handle_thumb_rect(
    width: i32,
    height: i32,
    total_height: f64,
    scroll_y: f64,
    hover_progress: f64,
) -> Option<Rect> {
    let (_, y, _, thumb_height) = thumb_rect(width, height, total_height, scroll_y)?;
    let handle = handle_rect(width, height, hover_progress);

    Some(Rect {
        y,
        height: thumb_height,
        ..handle
    })
}

pub(in crate::ui) fn set_hover(
    area: &gtk::DrawingArea,
    hover_cell: &Rc<Cell<bool>>,
    active_cell: &Rc<Cell<bool>>,
    progress_cell: &Rc<Cell<f64>>,
    animating_cell: &Rc<Cell<bool>>,
    hover: bool,
) {
    if hover_cell.get() == hover {
        return;
    }
    hover_cell.set(hover);
    start_hover_animation(area, hover_cell, active_cell, progress_cell, animating_cell);
}

pub(in crate::ui) fn set_active(
    area: &gtk::DrawingArea,
    hover_cell: &Rc<Cell<bool>>,
    active_cell: &Rc<Cell<bool>>,
    progress_cell: &Rc<Cell<f64>>,
    animating_cell: &Rc<Cell<bool>>,
    active: bool,
) {
    if active_cell.get() == active {
        return;
    }
    active_cell.set(active);
    start_hover_animation(area, hover_cell, active_cell, progress_cell, animating_cell);
}

fn start_hover_animation(
    area: &gtk::DrawingArea,
    hover_cell: &Rc<Cell<bool>>,
    active_cell: &Rc<Cell<bool>>,
    progress_cell: &Rc<Cell<f64>>,
    animating_cell: &Rc<Cell<bool>>,
) {
    if animating_cell.get() {
        return;
    }
    animating_cell.set(true);

    let area = area.clone();
    let hover_cell = hover_cell.clone();
    let active_cell = active_cell.clone();
    let progress_cell = progress_cell.clone();
    let animating_cell = animating_cell.clone();
    let step = (HOVER_ANIMATION_FRAME_MS / HOVER_ANIMATION_DURATION_MS).min(1.0);
    gtk::glib::timeout_add_local(
        Duration::from_millis(HOVER_ANIMATION_FRAME_MS as u64),
        move || {
            let target = if hover_cell.get() || active_cell.get() {
                1.0
            } else {
                0.0
            };
            let current = progress_cell.get();
            let delta = target - current;

            if delta.abs() < 0.02 {
                progress_cell.set(target);
                animating_cell.set(false);
                area.queue_draw();
                return gtk::glib::ControlFlow::Break;
            }

            let clamped_delta = if delta.is_sign_positive() {
                delta.min(step)
            } else {
                delta.max(-step)
            };
            progress_cell.set((current + clamped_delta).clamp(0.0, 1.0));
            area.queue_draw();
            gtk::glib::ControlFlow::Continue
        },
    );
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + (end - start) * amount.clamp(0.0, 1.0)
}

fn lerp_color(start: (f64, f64, f64), end: (f64, f64, f64), amount: f64) -> (f64, f64, f64) {
    (
        lerp(start.0, end.0, amount),
        lerp(start.1, end.1, amount),
        lerp(start.2, end.2, amount),
    )
}

fn fill_rect_rgba(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color: (f64, f64, f64),
    alpha: f64,
) {
    context.set_source_rgba(color.0, color.1, color.2, alpha);
    context.rectangle(x, y, width, height);
    let _ = context.fill();
}

fn fill_rounded_rect(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
    color: Color,
) {
    rounded_rect(context, x, y, width, height, radius);
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
    let _ = context.fill();
}

fn rounded_rect(context: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    context.close_path();
}
