pub trait CanvasPainter {
    fn save(&self);
    fn restore(&self);
    fn set_source_rgba(&self, red: f64, green: f64, blue: f64, alpha: f64);
    fn set_line_width(&self, width: f64);
    fn rectangle(&self, x: f64, y: f64, width: f64, height: f64);
    fn move_to(&self, x: f64, y: f64);
    fn line_to(&self, x: f64, y: f64);
    fn close_path(&self);
    fn new_sub_path(&self);
    fn arc(&self, x: f64, y: f64, radius: f64, start: f64, end: f64);
    fn fill(&self);
    fn fill_preserve(&self);
    fn stroke(&self);
    fn clip(&self);
}
