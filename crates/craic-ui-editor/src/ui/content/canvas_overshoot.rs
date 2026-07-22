use super::skia_canvas;
use gtk::prelude::*;
use skia_safe::{
    Color4f, Paint, TileMode,
    gradient::{Colors, Gradient, Interpolation, shaders},
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const MAX_OVERSHOOT_DISTANCE: f64 = 100.0;
const DECAY_FACTOR: f64 = 0.80;
const FRAME_MS: u64 = 16;
const GLOW_COLOR: (f64, f64, f64) = (0.92, 0.94, 0.98);

#[derive(Clone)]
pub struct EdgeGlow {
    top: Rc<Cell<f64>>,
    bottom: Rc<Cell<f64>>,
    left: Rc<Cell<f64>>,
    right: Rc<Cell<f64>>,
    animating: Rc<Cell<bool>>,
}

#[derive(Clone, Copy)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl EdgeGlow {
    pub fn new() -> Self {
        Self {
            top: Rc::new(Cell::new(0.0)),
            bottom: Rc::new(Cell::new(0.0)),
            left: Rc::new(Cell::new(0.0)),
            right: Rc::new(Cell::new(0.0)),
            animating: Rc::new(Cell::new(false)),
        }
    }

    pub fn pull(&self, area: &gtk::GLArea, edge: Edge, overflow: f64) {
        if overflow <= f64::EPSILON {
            return;
        }

        self.clear_opposite(edge);
        let impulse = (overflow * 0.72).clamp(2.0, 42.0);
        let cell = self.cell(edge);
        cell.set((cell.get() + impulse).min(MAX_OVERSHOOT_DISTANCE));
        area.queue_render();
        self.start_decay(area);
    }

    fn cell(&self, edge: Edge) -> &Cell<f64> {
        match edge {
            Edge::Top => &self.top,
            Edge::Bottom => &self.bottom,
            Edge::Left => &self.left,
            Edge::Right => &self.right,
        }
    }

    fn clear_opposite(&self, edge: Edge) {
        match edge {
            Edge::Top => self.bottom.set(0.0),
            Edge::Bottom => self.top.set(0.0),
            Edge::Left => self.right.set(0.0),
            Edge::Right => self.left.set(0.0),
        }
    }

    fn start_decay(&self, area: &gtk::GLArea) {
        if self.animating.get() {
            return;
        }
        self.animating.set(true);

        let area = area.clone();
        let state = self.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(FRAME_MS), move || {
            let active = decay_cell(&state.top)
                | decay_cell(&state.bottom)
                | decay_cell(&state.left)
                | decay_cell(&state.right);

            area.queue_render();
            if active {
                gtk::glib::ControlFlow::Continue
            } else {
                state.animating.set(false);
                gtk::glib::ControlFlow::Break
            }
        });
    }
}

pub fn pull_for_delta(
    area: &gtk::GLArea,
    state: &EdgeGlow,
    current: f64,
    max: f64,
    delta: f64,
    lower_edge: Edge,
    upper_edge: Edge,
) {
    if delta.abs() <= f64::EPSILON {
        return;
    }

    let max = max.max(0.0);
    let desired = current + delta;
    let overflow = if max <= f64::EPSILON {
        delta.abs()
    } else if desired < 0.0 {
        -desired
    } else if desired > max {
        desired - max
    } else {
        0.0
    };

    if overflow <= f64::EPSILON {
        return;
    }

    state.pull(
        area,
        if delta < 0.0 { lower_edge } else { upper_edge },
        overflow,
    );
}

pub fn draw(context: &skia_canvas::Context, width: i32, height: i32, state: &EdgeGlow) {
    if width <= 0 || height <= 0 {
        return;
    }

    let width = width as f64;
    let height = height as f64;
    draw_vertical_edge(context, width, height, state.top.get(), true);
    draw_vertical_edge(context, width, height, state.bottom.get(), false);
    draw_horizontal_edge(context, width, height, state.left.get(), true);
    draw_horizontal_edge(context, width, height, state.right.get(), false);
}

fn draw_vertical_edge(
    context: &skia_canvas::Context,
    width: f64,
    height: f64,
    distance: f64,
    top: bool,
) {
    let distance = distance.clamp(0.0, MAX_OVERSHOOT_DISTANCE).min(height);
    if distance <= 0.5 {
        return;
    }

    let strength = (distance / MAX_OVERSHOOT_DISTANCE).sqrt();
    let broad = (distance * 0.50).max(1.0);
    let narrow = (distance * 0.03).clamp(2.0, 5.0).min(distance);

    if top {
        fill_edge_gradient(
            context,
            (0.0, 0.0, width, broad),
            (width / 2.0, 0.0),
            (width / 2.0, broad),
            0.075 * strength,
        );
        fill_edge_gradient(
            context,
            (0.0, 0.0, width, narrow),
            (width / 2.0, 0.0),
            (width / 2.0, narrow),
            0.180 * strength,
        );
    } else {
        fill_edge_gradient(
            context,
            (0.0, height - broad, width, broad),
            (width / 2.0, height),
            (width / 2.0, broad),
            0.075 * strength,
        );
        fill_edge_gradient(
            context,
            (0.0, height - narrow, width, narrow),
            (width / 2.0, height),
            (width / 2.0, narrow),
            0.180 * strength,
        );
    }
}

fn draw_horizontal_edge(
    context: &skia_canvas::Context,
    width: f64,
    height: f64,
    distance: f64,
    left: bool,
) {
    let distance = distance.clamp(0.0, MAX_OVERSHOOT_DISTANCE).min(width);
    if distance <= 0.5 {
        return;
    }

    let strength = (distance / MAX_OVERSHOOT_DISTANCE).sqrt();
    let broad = (distance * 0.50).max(1.0);
    let narrow = (distance * 0.03).clamp(2.0, 5.0).min(distance);

    if left {
        fill_edge_gradient(
            context,
            (0.0, 0.0, broad, height),
            (0.0, height / 2.0),
            (broad, height / 2.0),
            0.075 * strength,
        );
        fill_edge_gradient(
            context,
            (0.0, 0.0, narrow, height),
            (0.0, height / 2.0),
            (narrow, height / 2.0),
            0.180 * strength,
        );
    } else {
        fill_edge_gradient(
            context,
            (width - broad, 0.0, broad, height),
            (width, height / 2.0),
            (broad, height / 2.0),
            0.075 * strength,
        );
        fill_edge_gradient(
            context,
            (width - narrow, 0.0, narrow, height),
            (width, height / 2.0),
            (narrow, height / 2.0),
            0.180 * strength,
        );
    }
}

fn fill_edge_gradient(
    context: &skia_canvas::Context,
    rect: (f64, f64, f64, f64),
    center: (f64, f64),
    radius: (f64, f64),
    alpha: f64,
) {
    let (x, y, width, height) = rect;
    let (center_x, center_y) = center;
    let (radius_x, radius_y) = radius;
    if width <= 0.0 || height <= 0.0 || radius_x <= 0.0 || radius_y <= 0.0 {
        return;
    }

    let _ = context.save();
    context.rectangle(x, y, width, height);
    context.clip();
    context.translate(center_x, center_y);
    context.scale(radius_x, radius_y);

    let colors = [
        glow_color(alpha),
        glow_color(alpha * 0.72),
        glow_color(alpha * 0.28),
        glow_color(alpha * 0.08),
        glow_color(0.0),
    ];
    let positions = [0.0, 0.20, 0.48, 0.72, 1.0];
    let colors = Colors::new(&colors, Some(&positions), TileMode::Clamp, None);
    let gradient = Gradient::new(colors, Interpolation::default());
    if let Some(shader) = shaders::radial_gradient(((0.0, 0.0), 1.0), &gradient, None) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true).set_shader(shader);
        context.canvas().draw_paint(&paint);
    }
    let _ = context.restore();
}

fn glow_color(alpha: f64) -> Color4f {
    Color4f::new(
        GLOW_COLOR.0 as f32,
        GLOW_COLOR.1 as f32,
        GLOW_COLOR.2 as f32,
        alpha as f32,
    )
}

fn decay_cell(cell: &Cell<f64>) -> bool {
    let next = cell.get() * DECAY_FACTOR;
    if next < 0.5 {
        cell.set(0.0);
        false
    } else {
        cell.set(next);
        true
    }
}
