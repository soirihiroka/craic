use skia_safe::{Canvas, ClipOp, Color4f, Paint, PaintStyle, PathBuilder, Rect};
use std::cell::{Cell, RefCell};

use craic_ui_core::ui::canvas_painter::CanvasPainter;

pub struct Context<'a> {
    canvas: &'a Canvas,
    path: RefCell<PathBuilder>,
    color: Cell<Color4f>,
    line_width: Cell<f32>,
    paint_stack: RefCell<Vec<(Color4f, f32)>>,
}

impl<'a> Context<'a> {
    pub fn new(canvas: &'a Canvas) -> Self {
        Self {
            canvas,
            path: RefCell::new(PathBuilder::new()),
            color: Cell::new(Color4f::new(0.0, 0.0, 0.0, 1.0)),
            line_width: Cell::new(1.0),
            paint_stack: RefCell::new(Vec::new()),
        }
    }

    pub fn canvas(&self) -> &Canvas {
        self.canvas
    }

    pub fn save(&self) -> Result<(), ()> {
        self.canvas.save();
        self.paint_stack
            .borrow_mut()
            .push((self.color.get(), self.line_width.get()));
        Ok(())
    }

    pub fn restore(&self) -> Result<(), ()> {
        self.canvas.restore();
        if let Some((color, line_width)) = self.paint_stack.borrow_mut().pop() {
            self.color.set(color);
            self.line_width.set(line_width);
        }
        Ok(())
    }

    pub fn set_source_rgba(&self, red: f64, green: f64, blue: f64, alpha: f64) {
        self.color.set(Color4f::new(
            red as f32,
            green as f32,
            blue as f32,
            alpha as f32,
        ));
    }

    pub fn set_line_width(&self, width: f64) {
        self.line_width.set(width as f32);
    }

    pub fn rectangle(&self, x: f64, y: f64, width: f64, height: f64) {
        self.path.borrow_mut().add_rect(
            Rect::from_xywh(x as f32, y as f32, width as f32, height as f32),
            None,
            None,
        );
    }

    pub fn move_to(&self, x: f64, y: f64) {
        self.path.borrow_mut().move_to((x as f32, y as f32));
    }

    pub fn line_to(&self, x: f64, y: f64) {
        self.path.borrow_mut().line_to((x as f32, y as f32));
    }

    pub fn close_path(&self) {
        self.path.borrow_mut().close();
    }

    pub fn new_sub_path(&self) {
        // A following move or arc starts a new Skia contour naturally.
    }

    pub fn arc(&self, x: f64, y: f64, radius: f64, start: f64, end: f64) {
        let diameter = radius * 2.0;
        self.path.borrow_mut().arc_to(
            Rect::from_xywh(
                (x - radius) as f32,
                (y - radius) as f32,
                diameter as f32,
                diameter as f32,
            ),
            start.to_degrees() as f32,
            (end - start).to_degrees() as f32,
            false,
        );
    }

    pub fn fill(&self) -> Result<(), ()> {
        self.draw_path(false, PaintStyle::Fill);
        Ok(())
    }

    fn draw_path(&self, preserve: bool, style: PaintStyle) {
        let path = if preserve {
            self.path.borrow().snapshot()
        } else {
            self.path.borrow_mut().detach()
        };
        let mut paint = Paint::new(self.color.get(), None);
        paint.set_anti_alias(true);
        paint.set_style(style);
        paint.set_stroke_width(self.line_width.get());
        self.canvas.draw_path(&path, &paint);
    }

    pub fn stroke(&self) -> Result<(), ()> {
        self.draw_path(false, PaintStyle::Stroke);
        Ok(())
    }

    pub fn clip(&self) {
        let path = self.path.borrow_mut().detach();
        self.canvas.clip_path(&path, ClipOp::Intersect, true);
    }

    pub fn clip_extents(&self) -> Result<(f64, f64, f64, f64), ()> {
        self.canvas
            .local_clip_bounds()
            .map(|rect| {
                (
                    rect.left as f64,
                    rect.top as f64,
                    rect.right as f64,
                    rect.bottom as f64,
                )
            })
            .ok_or(())
    }

    pub fn translate(&self, x: f64, y: f64) {
        self.canvas.translate((x as f32, y as f32));
    }

    pub fn rotate(&self, radians: f64) {
        self.canvas.rotate(radians.to_degrees() as f32, None);
    }

    pub fn scale(&self, x: f64, y: f64) {
        self.canvas.scale((x as f32, y as f32));
    }
}

impl CanvasPainter for Context<'_> {
    fn save(&self) {
        let _ = Context::save(self);
    }

    fn restore(&self) {
        let _ = Context::restore(self);
    }

    fn set_source_rgba(&self, red: f64, green: f64, blue: f64, alpha: f64) {
        Context::set_source_rgba(self, red, green, blue, alpha);
    }

    fn set_line_width(&self, width: f64) {
        Context::set_line_width(self, width);
    }

    fn rectangle(&self, x: f64, y: f64, width: f64, height: f64) {
        Context::rectangle(self, x, y, width, height);
    }

    fn move_to(&self, x: f64, y: f64) {
        Context::move_to(self, x, y);
    }

    fn line_to(&self, x: f64, y: f64) {
        Context::line_to(self, x, y);
    }

    fn close_path(&self) {
        Context::close_path(self);
    }

    fn new_sub_path(&self) {
        Context::new_sub_path(self);
    }

    fn arc(&self, x: f64, y: f64, radius: f64, start: f64, end: f64) {
        Context::arc(self, x, y, radius, start, end);
    }

    fn fill(&self) {
        self.draw_path(false, PaintStyle::Fill);
    }

    fn fill_preserve(&self) {
        self.draw_path(true, PaintStyle::Fill);
    }

    fn stroke(&self) {
        self.draw_path(false, PaintStyle::Stroke);
    }

    fn clip(&self) {
        Context::clip(self);
    }
}
