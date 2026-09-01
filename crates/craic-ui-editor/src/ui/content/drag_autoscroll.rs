pub(super) const ZONE_LINES: f64 = 2.0;

pub(super) fn lines_per_frame(ratio: f64) -> f64 {
    let ramp_ratio = ratio.clamp(0.0, 1.0);
    let outside_ratio = (ratio - 1.0).max(0.0);
    0.5 + ramp_ratio * 1.5 + ramp_ratio.powi(3) * 2.0 + outside_ratio * 2.0
}
