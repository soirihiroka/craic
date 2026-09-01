use crate::TextSyntaxSpan;
use craic_text_layout::{
    FoldRange, VisualLine, build_visual_lines_monospace, column_slice, text_columns,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorLineCommentEdit {
    pub range_start: usize,
    pub range_end: usize,
    pub replacement: String,
    pub uncomment: bool,
    prefix_edits: Vec<EditorLinePrefixEdit>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EditorLinePrefixEdit {
    start: usize,
    removed: usize,
    inserted: usize,
}

impl EditorLineCommentEdit {
    pub fn map_offset(&self, offset: usize) -> usize {
        let mut adjustment = 0isize;
        for edit in &self.prefix_edits {
            if offset < edit.start {
                break;
            }
            let adjusted_start = edit.start.saturating_add_signed(adjustment);
            let removed_end = edit.start.saturating_add(edit.removed);
            if offset <= removed_end {
                return adjusted_start.saturating_add(edit.inserted);
            }
            adjustment += edit.inserted as isize - edit.removed as isize;
        }
        offset.saturating_add_signed(adjustment)
    }

    pub fn map_selection(&self, selection: EditorSelection) -> EditorSelection {
        EditorSelection {
            anchor: self.map_offset(selection.anchor),
            focus: self.map_offset(selection.focus),
        }
    }

    pub fn line_count(&self) -> usize {
        self.prefix_edits.len()
    }
}

pub fn toggle_editor_line_comment(
    text: &str,
    selection: EditorSelection,
    prefix: &str,
) -> Option<EditorLineCommentEdit> {
    if prefix.is_empty() {
        return None;
    }
    let selection = EditorSelection {
        anchor: clamp_editor_offset(text, selection.anchor),
        focus: clamp_editor_offset(text, selection.focus),
    };
    let (start, end) = selection.normalized();
    let first_line_start = editor_line_start(text, start);
    let last_offset = if end > start {
        previous_editor_char_boundary(text, end)
    } else {
        start
    };
    let last_line_start = editor_line_start(text, last_offset);
    let range_start = first_line_start;
    let range_end = editor_line_end(text, last_line_start);
    let line_starts = editor_line_starts(text, first_line_start, last_line_start);
    let uncomment = should_uncomment_editor_lines(text, &line_starts, prefix);
    let mut replacement = String::with_capacity(
        range_end
            .saturating_sub(range_start)
            .saturating_add(line_starts.len() * (prefix.len() + 1)),
    );
    let mut prefix_edits = Vec::new();
    let mut copied_until = range_start;

    for line_start in line_starts {
        let line_end = editor_line_end(text, line_start);
        let line = &text[line_start..line_end];
        let indent_len = editor_leading_whitespace(line).len();
        let edit_start = line_start + indent_len;
        replacement.push_str(&text[copied_until..edit_start]);
        if uncomment {
            if let Some(removed) = editor_line_comment_remove_len(&line[indent_len..], prefix) {
                prefix_edits.push(EditorLinePrefixEdit {
                    start: edit_start,
                    removed,
                    inserted: 0,
                });
                copied_until = edit_start + removed;
            } else {
                copied_until = edit_start;
            }
        } else {
            replacement.push_str(prefix);
            replacement.push(' ');
            prefix_edits.push(EditorLinePrefixEdit {
                start: edit_start,
                removed: 0,
                inserted: prefix.len() + 1,
            });
            copied_until = edit_start;
        }
    }
    if prefix_edits.is_empty() {
        return None;
    }
    replacement.push_str(&text[copied_until..range_end]);
    Some(EditorLineCommentEdit {
        range_start,
        range_end,
        replacement,
        uncomment,
        prefix_edits,
    })
}

fn should_uncomment_editor_lines(text: &str, line_starts: &[usize], prefix: &str) -> bool {
    let mut has_nonblank_line = false;
    for &line_start in line_starts {
        let line_end = editor_line_end(text, line_start);
        let line = &text[line_start..line_end];
        if line.trim().is_empty() {
            continue;
        }
        has_nonblank_line = true;
        let indent_len = editor_leading_whitespace(line).len();
        if editor_line_comment_remove_len(&line[indent_len..], prefix).is_none() {
            return false;
        }
    }
    has_nonblank_line
}

fn editor_line_comment_remove_len(line_after_indent: &str, prefix: &str) -> Option<usize> {
    let rest = line_after_indent.strip_prefix(prefix)?;
    if prefix == "#" && rest.starts_with('!') {
        return None;
    }
    Some(
        prefix.len()
            + rest
                .chars()
                .next()
                .filter(|character| matches!(character, ' ' | '\t'))
                .map(char::len_utf8)
                .unwrap_or(0),
    )
}

fn editor_line_starts(text: &str, first: usize, last: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut line_start = first.min(text.len());
    loop {
        starts.push(line_start);
        if line_start >= last {
            break;
        }
        let line_end = editor_line_end(text, line_start);
        if line_end >= text.len() {
            break;
        }
        line_start = line_end + 1;
    }
    starts
}

fn editor_line_start(text: &str, offset: usize) -> usize {
    let offset = clamp_editor_offset(text, offset);
    text[..offset]
        .rfind('\n')
        .map(|newline| newline + 1)
        .unwrap_or(0)
}

fn editor_line_end(text: &str, offset: usize) -> usize {
    let offset = clamp_editor_offset(text, offset);
    text[offset..]
        .find('\n')
        .map(|newline| offset + newline)
        .unwrap_or(text.len())
}

fn previous_editor_char_boundary(text: &str, offset: usize) -> usize {
    let offset = clamp_editor_offset(text, offset);
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(byte, _)| byte)
        .unwrap_or(0)
}

fn clamp_editor_offset(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn editor_leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .take_while(|(_, character)| matches!(character, ' ' | '\t'))
        .map(|(offset, character)| offset + character.len_utf8())
        .last()
        .unwrap_or(0);
    &line[..end]
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
        document: &mut EditorDocument,
        metrics: EditorMetrics,
        trailing_inset: f64,
    ) {
        self.width = width.max(0.0);
        self.height = height.max(0.0);
        document.reflow(
            (self.width
                - metrics.gutter_width as f64
                - metrics.text_inset as f64 * 2.0
                - trailing_inset.max(0.0))
                / metrics.char_width.max(1.0) as f64,
        );
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
        let line = document.visual_line_for_offset(offset);
        let column = document.visual_column_for_offset(offset);
        let x = metrics.gutter_width as f64
            + metrics.text_inset as f64
            + column * metrics.char_width as f64;
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
        let start = document
            .visual_lines()
            .get(start_line.min(document.visual_lines().len().saturating_sub(1)))
            .map_or(0, |line| line.start);
        let end = document
            .visual_lines()
            .get(end_line.min(document.visual_lines().len().saturating_sub(1)))
            .map_or(document.text().len(), |line| line.end);
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
    visual_lines: Vec<VisualLine>,
    longest_visual_columns: usize,
    wrap_columns: f64,
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
        let shared_folds = folds
            .iter()
            .map(|fold| FoldRange {
                start_line: fold.start_line,
                end_line: fold.end_line,
                expanded: fold.expanded,
            })
            .collect::<Vec<_>>();
        let visual_lines = build_visual_lines_monospace(&text, &shared_folds, false, 1.0);
        Self {
            text,
            lines,
            syntax,
            folds,
            visual_lines,
            longest_visual_columns,
            wrap_columns: 0.0,
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

    pub fn visual_lines(&self) -> &[VisualLine] {
        &self.visual_lines
    }

    pub fn reflow(&mut self, wrap_columns: f64) {
        let wrap_columns = wrap_columns.floor().max(1.0);
        if (self.wrap_columns - wrap_columns).abs() < f64::EPSILON {
            return;
        }
        let folds = self
            .folds
            .iter()
            .map(|fold| FoldRange {
                start_line: fold.start_line,
                end_line: fold.end_line,
                expanded: fold.expanded,
            })
            .collect::<Vec<_>>();
        self.visual_lines = build_visual_lines_monospace(&self.text, &folds, true, wrap_columns);
        self.longest_visual_columns = self
            .visual_lines
            .iter()
            .map(|line| text_columns(&self.text[line.start..line.end]).ceil() as usize)
            .max()
            .unwrap_or(0);
        self.wrap_columns = wrap_columns;
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
        craic_text_layout::visual_line_index_for_offset(&self.visual_lines, offset)
    }

    pub fn visual_column_for_offset(&self, offset: usize) -> f64 {
        let offset = self.clamp_offset(offset);
        let visual_line = self.visual_line_for_offset(offset);
        self.visual_lines
            .get(visual_line)
            .and_then(|line| {
                self.text
                    .get(line.start..offset.clamp(line.start, line.end))
            })
            .map_or(0.0, text_columns)
    }

    pub fn offset_for_visual_line_column(&self, visual_line: usize, visual_column: f64) -> usize {
        let Some(line) = self.visual_lines.get(visual_line) else {
            return self.text.len();
        };
        let text = &self.text[line.start..line.end];
        line.start + column_slice(text, visual_column, visual_column + 1.0).start
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
        let line_height = metrics.line_height as f64;
        let text_start = metrics.text_inset as f64;
        let start = ((scroll_y - text_start).max(0.0) / line_height).floor() as usize;
        let end =
            ((scroll_y + viewport_height - text_start).max(0.0) / line_height).ceil() as usize;
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
        let visual_line = (((y + scroll_y - metrics.text_inset as f64).max(0.0)
            / metrics.line_height as f64)
            .floor() as usize)
            .min(self.visual_lines.len().saturating_sub(1));
        if self.visual_lines.is_empty() {
            return self.text.len();
        }
        let column = ((x + scroll_x - metrics.gutter_width as f64 - metrics.text_inset as f64)
            .max(0.0)
            / metrics.char_width as f64)
            .round() as usize;
        self.offset_for_visual_line_column(visual_line, column as f64)
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
    text_columns(text).ceil() as usize
}
