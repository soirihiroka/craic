use super::{VisualLine, canvas};
use crate::language_support::{HighlightRange, SyntaxHighlighter};
use crate::spellcheck::{SpellcheckAllowlist, check_document};
use skia_safe::{Color, Surface, surfaces};

const FONT_SIZE: f64 = 14.0;
const VIEWPORT_WIDTH: f64 = 1280.0;
const GUTTER_WIDTH: f64 = 64.0;
const VISIBLE_LINES: usize = 36;

pub struct OffscreenDocument {
    source: String,
    highlights: Vec<HighlightRange>,
    visual_lines: Vec<VisualLine>,
    surface: Surface,
}

impl OffscreenDocument {
    pub fn new(source: String) -> Self {
        let mut highlighter = SyntaxHighlighter::new("csv");
        highlighter.set_source(&source);
        let highlights = highlighter.highlight_current();
        let surface = surfaces::raster_n32_premul((1280, 720)).expect("raster benchmark surface");
        Self {
            source,
            highlights,
            visual_lines: Vec::new(),
            surface,
        }
    }

    pub fn measured_layout(&mut self, wrap: bool) -> usize {
        self.visual_lines = craic_text_layout::build_visual_lines_monospace(
            &self.source,
            &[],
            wrap,
            (VIEWPORT_WIDTH - GUTTER_WIDTH) / 8.0,
        );
        self.visual_lines.len()
    }

    pub fn paint_plain(&mut self, first_line: usize) {
        self.ensure_layout();
        self.surface.canvas().clear(Color::BLACK);
        let context = super::super::skia_canvas::Context::new(self.surface.canvas());
        for (row, line) in self
            .visual_lines
            .iter()
            .skip(first_line.min(self.visual_lines.len()))
            .take(VISIBLE_LINES)
            .enumerate()
        {
            canvas::draw_plain_text_headless(
                &context,
                FONT_SIZE,
                &self.source[line.start..line.end],
                GUTTER_WIDTH,
                20.0 + row as f64 * 19.0,
                canvas::TextColor::rgb(0.86, 0.86, 0.86),
            );
        }
    }

    pub fn paint_highlighted(&mut self, first_line: usize) {
        self.ensure_layout();
        self.surface.canvas().clear(Color::BLACK);
        let context = super::super::skia_canvas::Context::new(self.surface.canvas());
        for (row, line) in self
            .visual_lines
            .iter()
            .skip(first_line.min(self.visual_lines.len()))
            .take(VISIBLE_LINES)
            .enumerate()
        {
            let baseline = 20.0 + row as f64 * 19.0;
            let mut cursor = line.start;
            let mut runs = Vec::new();
            let first_range = self
                .highlights
                .partition_point(|range| range.end <= line.start);
            for range in &self.highlights[first_range..] {
                if range.start >= line.end {
                    break;
                }
                let start = range.start.max(line.start).max(cursor);
                let end = range.end.min(line.end);
                if start >= end {
                    continue;
                }
                if cursor < start {
                    let plain = &self.source[cursor..start];
                    runs.push(canvas::StyledText {
                        text: plain,
                        color: canvas::TextColor::rgb(0.86, 0.86, 0.86),
                    });
                }
                let segment = &self.source[start..end];
                let (red, green, blue) = range.style.color();
                runs.push(canvas::StyledText {
                    text: segment,
                    color: canvas::TextColor::rgb(red, green, blue),
                });
                cursor = end;
            }
            if cursor < line.end {
                runs.push(canvas::StyledText {
                    text: &self.source[cursor..line.end],
                    color: canvas::TextColor::rgb(0.86, 0.86, 0.86),
                });
            }
            canvas::draw_styled_text_headless(&context, FONT_SIZE, &runs, GUTTER_WIDTH, baseline);
        }
    }

    pub fn project_dense_markers(&mut self) -> usize {
        self.ensure_layout();
        craic_text_layout::visual_spans(&self.visual_lines, self.source.lines().count())
            .into_iter()
            .flatten()
            .map(|(first, count)| first + count)
            .sum()
    }

    pub fn visual_line_count(&mut self) -> usize {
        self.ensure_layout();
        self.visual_lines.len()
    }

    fn ensure_layout(&mut self) {
        if self.visual_lines.is_empty() {
            self.measured_layout(true);
        }
    }
}

pub fn highlight_csv(source: &str) -> usize {
    let mut highlighter = SyntaxHighlighter::new("csv");
    highlighter.set_source(source);
    highlighter.highlight_current().len()
}

pub fn spellcheck_csv(source: &str) -> usize {
    check_document(
        "csv",
        Some("benchmark.csv"),
        source,
        &SpellcheckAllowlist::default(),
    )
    .len()
}

pub fn generated_csv(rows: usize) -> String {
    let mut source = String::from("id,p0_x,p0_y,p1_x,p1_y,p2_x,p2_y,p3_x,p3_y\n");
    for row in 0..rows {
        source.push_str(&format!(
            "0-{row},{:.5},{:.4},{:.5},{:.4},{:.5},{:.4},{:.5},{:.4}\n",
            10.0 + row as f64 * 0.013,
            300.0 + row as f64 * 0.017,
            20.0 + row as f64 * 0.019,
            310.0 + row as f64 * 0.023,
            30.0 + row as f64 * 0.029,
            320.0 + row as f64 * 0.031,
            40.0 + row as f64 * 0.037,
            330.0 + row as f64 * 0.041,
        ));
    }
    source
}
