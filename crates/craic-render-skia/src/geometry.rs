#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionRange {
    pub anchor: usize,
    pub focus: usize,
}

impl SelectionRange {
    pub fn ordered(self) -> std::ops::Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollGeometry {
    pub content_width: f64,
    pub content_height: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub x: f64,
    pub y: f64,
}

impl ScrollGeometry {
    pub fn clamp(mut self) -> Self {
        self.x = self
            .x
            .clamp(0.0, (self.content_width - self.viewport_width).max(0.0));
        self.y = self
            .y
            .clamp(0.0, (self.content_height - self.viewport_height).max(0.0));
        self
    }
}
