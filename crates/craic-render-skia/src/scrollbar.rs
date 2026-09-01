use crate::Rect;

pub const VERTICAL_SCROLLBAR_WIDTH: f64 = 24.0;
pub const VERTICAL_SCROLLBAR_MIN_THUMB: f64 = 40.0;
pub const VERTICAL_SCROLLBAR_VERTICAL_MARGIN: f64 = 9.0;

const IDLE_LANE_WIDTH: f64 = 11.0;
const IDLE_HANDLE_MARGIN: f64 = 4.0;
const HOVER_HANDLE_MARGIN: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalScrollbarLayout {
    pub track: Rect,
    pub handle: Rect,
    pub thumb: Rect,
    pub maximum: f64,
}

pub fn vertical_scrollbar_layout(
    viewport_width: f64,
    viewport_height: f64,
    total_height: f64,
    scroll_y: f64,
    hover_progress: f64,
) -> Option<VerticalScrollbarLayout> {
    if total_height <= viewport_height + 0.5 {
        return None;
    }

    let track = vertical_scrollbar_track_rect(viewport_width, viewport_height);
    let handle = vertical_scrollbar_handle_rect(viewport_width, viewport_height, hover_progress);
    let thumb_height = (track.height * viewport_height / total_height)
        .max(VERTICAL_SCROLLBAR_MIN_THUMB)
        .min(track.height);
    let maximum = (total_height - viewport_height).max(0.0);
    let travel = (track.height - thumb_height).max(0.0);
    let thumb_y = track.y + scroll_y.clamp(0.0, maximum) / maximum.max(1.0) * travel;
    let thumb = Rect {
        y: thumb_y,
        height: thumb_height,
        ..handle
    };

    Some(VerticalScrollbarLayout {
        track,
        handle,
        thumb,
        maximum,
    })
}

pub fn vertical_scrollbar_scroll_for_press(layout: VerticalScrollbarLayout, pointer_y: f64) -> f64 {
    if pointer_y >= layout.thumb.y && pointer_y <= layout.thumb.y + layout.thumb.height {
        return layout.thumb_position();
    }
    let travel = (layout.track.height - layout.thumb.height).max(0.0);
    if travel <= f64::EPSILON || layout.maximum <= f64::EPSILON {
        return 0.0;
    }
    let thumb_y =
        (pointer_y - layout.thumb.height / 2.0).clamp(layout.track.y, layout.track.y + travel);
    (thumb_y - layout.track.y) / travel * layout.maximum
}

pub fn vertical_scrollbar_scroll_for_drag(
    layout: VerticalScrollbarLayout,
    start_scroll_y: f64,
    pointer_delta_y: f64,
) -> f64 {
    vertical_scrollbar_scroll_for_delta(
        start_scroll_y,
        pointer_delta_y,
        layout.track.height,
        layout.thumb.height,
        layout.maximum,
    )
}

pub fn vertical_scrollbar_track_rect(viewport_width: f64, viewport_height: f64) -> Rect {
    Rect {
        x: viewport_width - VERTICAL_SCROLLBAR_WIDTH,
        y: VERTICAL_SCROLLBAR_VERTICAL_MARGIN,
        width: VERTICAL_SCROLLBAR_WIDTH,
        height: (viewport_height - VERTICAL_SCROLLBAR_VERTICAL_MARGIN * 2.0).max(1.0),
    }
}

pub fn vertical_scrollbar_handle_rect(
    viewport_width: f64,
    viewport_height: f64,
    hover_progress: f64,
) -> Rect {
    let track = vertical_scrollbar_track_rect(viewport_width, viewport_height);
    let hover_progress = hover_progress.clamp(0.0, 1.0);
    let lane_width =
        IDLE_LANE_WIDTH + (VERTICAL_SCROLLBAR_WIDTH - IDLE_LANE_WIDTH) * hover_progress;
    let margin = IDLE_HANDLE_MARGIN + (HOVER_HANDLE_MARGIN - IDLE_HANDLE_MARGIN) * hover_progress;
    Rect {
        x: viewport_width - lane_width + margin,
        y: track.y,
        width: (lane_width - margin * 2.0).max(1.0),
        height: track.height,
    }
}

pub fn vertical_scrollbar_scroll_for_delta(
    start_scroll_y: f64,
    pointer_delta_y: f64,
    track_height: f64,
    thumb_height: f64,
    maximum: f64,
) -> f64 {
    let travel = (track_height - thumb_height).max(1.0);
    (start_scroll_y + pointer_delta_y / travel * maximum).clamp(0.0, maximum)
}

impl VerticalScrollbarLayout {
    fn thumb_position(self) -> f64 {
        let travel = (self.track.height - self.thumb.height).max(1.0);
        (self.thumb.y - self.track.y) / travel * self.maximum
    }
}
