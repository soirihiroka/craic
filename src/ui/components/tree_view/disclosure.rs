use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

const DISCLOSURE_SPEED: f64 = 12.0;

#[derive(Clone)]
struct DisclosureState {
    progress: f64,
    target: f64,
    updated_at: Instant,
}

impl DisclosureState {
    fn settled(target: f64) -> Self {
        Self {
            progress: target,
            target,
            updated_at: Instant::now(),
        }
    }
}

pub(in crate::ui) struct DisclosureAnimator<K> {
    states: RefCell<HashMap<K, DisclosureState>>,
}

impl<K> DisclosureAnimator<K>
where
    K: Clone + Eq + Hash,
{
    pub(in crate::ui) fn new() -> Self {
        Self {
            states: RefCell::new(HashMap::new()),
        }
    }

    pub(in crate::ui) fn prepare(&self, key: &K, expanded: bool) -> bool {
        let target = if expanded { 1.0 } else { 0.0 };
        let mut states = self.states.borrow_mut();
        let state = states
            .entry(key.clone())
            .or_insert_with(|| DisclosureState::settled(target));
        let should_animate =
            (state.target - target).abs() > f64::EPSILON || (state.progress - target).abs() > 0.001;
        if should_animate {
            state.target = target;
            state.updated_at = Instant::now();
        }
        should_animate
    }

    pub(in crate::ui) fn advance(&self, key: &K) -> bool {
        let mut states = self.states.borrow_mut();
        let Some(state) = states.get_mut(key) else {
            return true;
        };

        let now = Instant::now();
        let elapsed = now.duration_since(state.updated_at).as_secs_f64();
        state.updated_at = now;
        let distance = state.target - state.progress;
        if distance.abs() <= 0.001 {
            state.progress = state.target;
            return true;
        }

        let step = (elapsed * DISCLOSURE_SPEED).clamp(0.0, 1.0);
        state.progress += distance * step;
        if (state.target - state.progress).abs() <= 0.001 {
            state.progress = state.target;
            true
        } else {
            false
        }
    }

    pub(in crate::ui) fn draw(
        &self,
        key: &K,
        area: &gtk::DrawingArea,
        context: &gtk::cairo::Context,
        width: i32,
        height: i32,
    ) {
        let progress = self
            .states
            .borrow()
            .get(key)
            .map(|state| state.progress)
            .unwrap_or_default()
            .clamp(0.0, 1.0);
        draw_disclosure(area, context, width, height, progress);
    }
}

fn draw_disclosure(
    area: &gtk::DrawingArea,
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    progress: f64,
) {
    let color = area.color();
    let width = width as f64;
    let height = height as f64;
    let size = width.min(height).max(1.0);
    let chevron = (size * 0.32).clamp(4.0, 6.0);

    let _ = context.save();
    context.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64,
    );
    context.set_line_width(1.7);
    context.set_line_cap(gtk::cairo::LineCap::Round);
    context.set_line_join(gtk::cairo::LineJoin::Round);
    context.translate(width / 2.0, height / 2.0);
    context.rotate(progress * std::f64::consts::FRAC_PI_2);
    context.move_to(-chevron / 2.0, -chevron);
    context.line_to(chevron / 2.0, 0.0);
    context.line_to(-chevron / 2.0, chevron);
    let _ = context.stroke();
    let _ = context.restore();
}
