pub const MIN_ZOOM: f64 = 0.05;
pub const MAX_ZOOM: f64 = 16.0;
pub const ZOOM_STEP: f64 = 1.1;

pub fn clamp_zoom(zoom: f64) -> f64 {
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

pub fn fit_scale(
    image_width: i32,
    image_height: i32,
    viewport_width: i32,
    viewport_height: i32,
) -> f64 {
    if image_width <= 0 || image_height <= 0 {
        return 1.0;
    }
    if viewport_width <= 0 || viewport_height <= 0 {
        return 1.0;
    }

    let image_width = image_width as f64;
    let image_height = image_height as f64;
    let viewport_width = viewport_width as f64;
    let viewport_height = viewport_height as f64;

    (viewport_width / image_width).min(viewport_height / image_height)
}
