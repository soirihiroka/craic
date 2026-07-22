use gtk::cairo;

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

impl CanvasPainter for cairo::Context {
    fn save(&self) {
        let _ = cairo::Context::save(self);
    }

    fn restore(&self) {
        let _ = cairo::Context::restore(self);
    }

    fn set_source_rgba(&self, red: f64, green: f64, blue: f64, alpha: f64) {
        cairo::Context::set_source_rgba(self, red, green, blue, alpha);
    }

    fn set_line_width(&self, width: f64) {
        cairo::Context::set_line_width(self, width);
    }

    fn rectangle(&self, x: f64, y: f64, width: f64, height: f64) {
        cairo::Context::rectangle(self, x, y, width, height);
    }

    fn move_to(&self, x: f64, y: f64) {
        cairo::Context::move_to(self, x, y);
    }

    fn line_to(&self, x: f64, y: f64) {
        cairo::Context::line_to(self, x, y);
    }

    fn close_path(&self) {
        cairo::Context::close_path(self);
    }

    fn new_sub_path(&self) {
        cairo::Context::new_sub_path(self);
    }

    fn arc(&self, x: f64, y: f64, radius: f64, start: f64, end: f64) {
        cairo::Context::arc(self, x, y, radius, start, end);
    }

    fn fill(&self) {
        let _ = cairo::Context::fill(self);
    }

    fn fill_preserve(&self) {
        let _ = cairo::Context::fill_preserve(self);
    }

    fn stroke(&self) {
        let _ = cairo::Context::stroke(self);
    }

    fn clip(&self) {
        cairo::Context::clip(self);
    }
}
