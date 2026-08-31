use crate::TextSyntaxSpan;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditorSelection {
    pub anchor: usize,
    pub focus: usize,
}

impl EditorSelection {
    pub fn normalized(self) -> (usize, usize) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EditorMetrics {
    pub font_size: f32,
    pub line_height: f32,
    pub char_width: f32,
    pub gutter_width: f32,
    pub text_inset: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorViewport {
    width: f64,
    height: f64,
    scroll_x: f64,
    scroll_y: f64,
}

impl EditorViewport {
    pub fn width(self) -> f64 {
        self.width
    }

    pub fn height(self) -> f64 {
        self.height
    }

    pub fn scroll_x(self) -> f64 {
        self.scroll_x
    }

    pub fn scroll_y(self) -> f64 {
        self.scroll_y
    }

    pub fn resize(
        &mut self,
        width: f64,
        height: f64,
        document: &EditorDocument,
        metrics: EditorMetrics,
    ) {
        self.width = width.max(0.0);
        self.height = height.max(0.0);
        self.clamp(document, metrics);
    }

    pub fn reset(&mut self) {
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
    }

    pub fn scroll_by(
        &mut self,
        delta_x: f64,
        delta_y: f64,
        document: &EditorDocument,
        metrics: EditorMetrics,
    ) {
        self.scroll_x += delta_x;
        self.scroll_y += delta_y;
        self.clamp(document, metrics);
    }

    pub fn autoscroll_for_pointer(
        &mut self,
        x: f64,
        y: f64,
        document: &EditorDocument,
        metrics: EditorMetrics,
    ) -> bool {
        let delta_x = if x < 0.0 {
            x.max(-48.0)
        } else if x > self.width {
            (x - self.width).min(48.0)
        } else {
            0.0
        };
        let delta_y = if y < 0.0 {
            y.max(-48.0)
        } else if y > self.height {
            (y - self.height).min(48.0)
        } else {
            0.0
        };
        if delta_x == 0.0 && delta_y == 0.0 {
            return false;
        }
        let before = (self.scroll_x, self.scroll_y);
        self.scroll_by(delta_x, delta_y, document, metrics);
        before != (self.scroll_x, self.scroll_y)
    }

    pub fn reveal_offset(
        &mut self,
        document: &EditorDocument,
        offset: usize,
        metrics: EditorMetrics,
        trailing_inset: f64,
    ) {
        let (_, column) = document.line_column_for_offset(offset);
        let line = document.visual_line_for_offset(offset);
        let x = metrics.gutter_width as f64
            + metrics.text_inset as f64
            + column as f64 * metrics.char_width as f64;
        let y = metrics.text_inset as f64 + line as f64 * metrics.line_height as f64;
        if x < self.scroll_x + metrics.gutter_width as f64 {
            self.scroll_x = (x - metrics.gutter_width as f64).max(0.0);
        } else if x > self.scroll_x + self.width - trailing_inset {
            self.scroll_x = x - self.width + trailing_inset + metrics.char_width as f64;
        }
        if y < self.scroll_y {
            self.scroll_y = y;
        } else if y + metrics.line_height as f64 > self.scroll_y + self.height {
            self.scroll_y = y + metrics.line_height as f64 - self.height;
        }
        self.clamp(document, metrics);
    }

    pub fn maximum_y(self, document: &EditorDocument, metrics: EditorMetrics) -> f64 {
        let (_, content_height) = document.content_size(metrics);
        (content_height as f64 - self.height).max(0.0)
    }

    pub fn source_offset(self, document: &EditorDocument, metrics: EditorMetrics) -> usize {
        let maximum = self.maximum_y(document, metrics);
        if maximum > 0.0 && self.scroll_y >= maximum - 0.5 {
            return document.text().len();
        }
        document.hit_test(
            metrics.gutter_width as f64 + metrics.text_inset as f64,
            0.0,
            self.scroll_x,
            self.scroll_y,
            metrics,
        )
    }

    pub fn visible_byte_range(
        self,
        document: &EditorDocument,
        metrics: EditorMetrics,
    ) -> std::ops::Range<usize> {
        let line_height = metrics.line_height.max(1.0) as f64;
        let start_line = (self.scroll_y / line_height).floor() as usize;
        let end_line = ((self.scroll_y + self.height) / line_height).ceil() as usize;
        let start_source = document
            .visual_lines()
            .get(start_line.min(document.visual_lines().len().saturating_sub(1)))
            .copied()
            .unwrap_or(0);
        let end_source = document
            .visual_lines()
            .get(end_line.min(document.visual_lines().len().saturating_sub(1)))
            .copied()
            .unwrap_or_else(|| document.lines().len().saturating_sub(1));
        let start = document
            .lines()
            .get(start_source)
            .map_or(0, |line| line.start);
        let end = document
            .lines()
            .get(end_source)
            .map_or(document.text().len(), |line| line.end_with_newline);
        start.min(end)..end.max(start)
    }

    fn clamp(&mut self, document: &EditorDocument, metrics: EditorMetrics) {
        let (content_width, content_height) = document.content_size(metrics);
        self.scroll_x = self
            .scroll_x
            .clamp(0.0, (content_width as f64 - self.width).max(0.0));
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_height as f64 - self.height).max(0.0));
    }
}

impl EditorMetrics {
    pub fn for_font_size(font_size: f32) -> Self {
        let font_size = font_size.clamp(8.0, 72.0);
        Self {
            font_size,
            line_height: font_size * 1.62,
            char_width: font_size * 0.602,
            gutter_width: 54.0,
            text_inset: 10.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EditorLine {
    pub start: usize,
    pub end: usize,
    pub end_with_newline: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorFoldRange {
    pub start_line: usize,
    pub end_line: usize,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
pub struct EditorDocument {
    text: String,
    lines: Vec<EditorLine>,
    syntax: Vec<TextSyntaxSpan>,
    folds: Vec<EditorFoldRange>,
    visual_lines: Vec<usize>,
    longest_visual_columns: usize,
}

impl Default for EditorDocument {
    fn default() -> Self {
        Self::new(String::new(), Vec::new())
    }
}

impl EditorDocument {
    pub fn new(text: String, syntax: Vec<TextSyntaxSpan>) -> Self {
        Self::new_with_folds(text, syntax, Vec::new())
    }

    pub fn new_with_folds(
        text: String,
        mut syntax: Vec<TextSyntaxSpan>,
        mut folds: Vec<EditorFoldRange>,
    ) -> Self {
        syntax.retain(|span| span.start < span.end && span.end <= text.len());
        syntax.sort_by_key(|span| (span.start, span.end));
        let mut lines = Vec::new();
        let mut start = 0;
        let mut longest_visual_columns = 0;
        for chunk in text.split_inclusive('\n') {
            let end_with_newline = start + chunk.len();
            let end = chunk
                .strip_suffix('\n')
                .map_or(end_with_newline, |without| start + without.len());
            longest_visual_columns = longest_visual_columns.max(visual_columns(&text[start..end]));
            lines.push(EditorLine {
                start,
                end,
                end_with_newline,
            });
            start = end_with_newline;
        }
        if text.is_empty() || text.ends_with('\n') {
            lines.push(EditorLine {
                start: text.len(),
                end: text.len(),
                end_with_newline: text.len(),
            });
        }
        folds.retain(|fold| fold.start_line < fold.end_line && fold.end_line < lines.len());
        folds.sort_by_key(|fold| (fold.start_line, fold.end_line));
        folds.dedup_by_key(|fold| (fold.start_line, fold.end_line));
        let mut visual_lines = Vec::with_capacity(lines.len());
        let mut source_line = 0;
        while source_line < lines.len() {
            visual_lines.push(source_line);
            let collapsed_end = folds
                .iter()
                .filter(|fold| !fold.expanded && fold.start_line == source_line)
                .map(|fold| fold.end_line)
                .max();
            source_line = collapsed_end.map_or(source_line + 1, |end| end + 1);
        }
        Self {
            text,
            lines,
            syntax,
            folds,
            visual_lines,
            longest_visual_columns,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn lines(&self) -> &[EditorLine] {
        &self.lines
    }

    pub fn syntax(&self) -> &[TextSyntaxSpan] {
        &self.syntax
    }

    pub fn folds(&self) -> &[EditorFoldRange] {
        &self.folds
    }

    pub fn visual_lines(&self) -> &[usize] {
        &self.visual_lines
    }

    pub fn fold_starting_at(&self, source_line: usize) -> Option<EditorFoldRange> {
        self.folds
            .iter()
            .copied()
            .filter(|fold| fold.start_line == source_line)
            .max_by_key(|fold| fold.end_line)
    }

    pub fn line_text(&self, line: usize) -> &str {
        self.lines
            .get(line)
            .and_then(|line| self.text.get(line.start..line.end))
            .unwrap_or_default()
    }

    pub fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub fn line_column_for_offset(&self, offset: usize) -> (usize, usize) {
        let offset = self.clamp_offset(offset);
        let line = self
            .lines
            .partition_point(|line| line.end_with_newline <= offset)
            .min(self.lines.len().saturating_sub(1));
        let line_start = self.lines.get(line).map_or(0, |line| line.start);
        let column = self.text.get(line_start..offset).map_or(0, visual_columns);
        (line, column)
    }

    pub fn visual_line_for_offset(&self, offset: usize) -> usize {
        let (source_line, _) = self.line_column_for_offset(offset);
        match self.visual_lines.binary_search(&source_line) {
            Ok(line) => line,
            Err(insertion) => insertion.saturating_sub(1),
        }
    }

    pub fn offset_for_line_column(&self, line: usize, visual_column: usize) -> usize {
        let Some(line) = self.lines.get(line) else {
            return self.text.len();
        };
        let text = &self.text[line.start..line.end];
        let mut columns = 0;
        for (byte, character) in text.char_indices() {
            let width = if character == '\t' { 4 } else { 1 };
            if columns + width / 2 >= visual_column {
                return line.start + byte;
            }
            columns += width;
        }
        line.end
    }

    pub fn content_size(&self, metrics: EditorMetrics) -> (f32, f32) {
        let width = metrics.gutter_width
            + metrics.text_inset * 2.0
            + self.longest_visual_columns as f32 * metrics.char_width;
        let height =
            self.visual_lines.len() as f32 * metrics.line_height + metrics.text_inset * 2.0;
        (width.max(1.0), height.max(1.0))
    }

    pub fn visible_line_range(
        &self,
        scroll_y: f64,
        viewport_height: f64,
        metrics: EditorMetrics,
    ) -> std::ops::Range<usize> {
        let overscan = metrics.line_height as f64 * 3.0;
        let start = ((scroll_y - overscan).max(0.0) / metrics.line_height as f64).floor() as usize;
        let end =
            ((scroll_y + viewport_height + overscan) / metrics.line_height as f64).ceil() as usize;
        start.min(self.visual_lines.len())..end.min(self.visual_lines.len())
    }

    pub fn hit_test(
        &self,
        x: f64,
        y: f64,
        scroll_x: f64,
        scroll_y: f64,
        metrics: EditorMetrics,
    ) -> usize {
        let visual_line = ((y + scroll_y - metrics.text_inset as f64).max(0.0)
            / metrics.line_height as f64)
            .floor() as usize;
        let line = self
            .visual_lines
            .get(visual_line.min(self.visual_lines.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0);
        let column = ((x + scroll_x - metrics.gutter_width as f64 - metrics.text_inset as f64)
            .max(0.0)
            / metrics.char_width as f64)
            .round() as usize;
        self.offset_for_line_column(line.min(self.lines.len().saturating_sub(1)), column)
    }
}

pub fn selected_editor_text(document: &EditorDocument, selection: EditorSelection) -> String {
    let (start, end) = selection.normalized();
    document
        .text()
        .get(document.clamp_offset(start)..document.clamp_offset(end))
        .unwrap_or_default()
        .to_string()
}

fn visual_columns(text: &str) -> usize {
    text.chars()
        .map(|character| if character == '\t' { 4 } else { 1 })
        .sum()
}
