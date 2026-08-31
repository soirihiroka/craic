use gtk::cairo;

pub use craic_render_skia::CanvasPainter;

pub struct CairoPainter<'a>(&'a cairo::Context);

impl<'a> CairoPainter<'a> {
    pub fn new(context: &'a cairo::Context) -> Self {
        Self(context)
    }
}

impl CanvasPainter for CairoPainter<'_> {
    fn save(&self) {
        let _ = self.0.save();
    }

    fn restore(&self) {
        let _ = self.0.restore();
    }

    fn set_source_rgba(&self, red: f64, green: f64, blue: f64, alpha: f64) {
        self.0.set_source_rgba(red, green, blue, alpha);
    }

    fn set_line_width(&self, width: f64) {
        self.0.set_line_width(width);
    }

    fn rectangle(&self, x: f64, y: f64, width: f64, height: f64) {
        self.0.rectangle(x, y, width, height);
    }

    fn move_to(&self, x: f64, y: f64) {
        self.0.move_to(x, y);
    }

    fn line_to(&self, x: f64, y: f64) {
        self.0.line_to(x, y);
    }

    fn close_path(&self) {
        self.0.close_path();
    }

    fn new_sub_path(&self) {
        self.0.new_sub_path();
    }

    fn arc(&self, x: f64, y: f64, radius: f64, start: f64, end: f64) {
        self.0.arc(x, y, radius, start, end);
    }

    fn fill(&self) {
        let _ = self.0.fill();
    }

    fn fill_preserve(&self) {
        let _ = self.0.fill_preserve();
    }

    fn stroke(&self) {
        let _ = self.0.stroke();
    }

    fn clip(&self) {
        self.0.clip();
    }
}
