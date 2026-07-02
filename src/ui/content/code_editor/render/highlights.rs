use super::theme::{Color, EditorTheme};
use super::{draw_plain_text, text_width};
use crate::language_support::{HighlightRange, SyntaxIssue};
use crate::spellcheck::SpellcheckIssue;
use crate::ui::content::code_editor::EditorState;
use gtk::cairo;
use std::rc::Rc;

pub(super) fn draw_highlighted_slice(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    source: &str,
    highlights: &[HighlightRange],
    start: usize,
    end: usize,
    mut x: f64,
    baseline: f64,
    max_x: f64,
    min_x: f64,
) {
    let mut cursor = start;
    let first_range = highlights.partition_point(|range| range.end <= start);
    for range in &highlights[first_range..] {
        if range.start >= end {
            break;
        }
        if !valid_highlight_range(source, range) {
            continue;
        }
        let range_start = range.start.max(start);
        let range_end = range.end.min(end);
        let range_start = range_start.max(cursor);
        if range_start >= range_end {
            continue;
        }
        if cursor < range_start {
            let plain = &source[cursor..range_start];
            draw_plain_text(area, context, state, plain, x, baseline, (0.86, 0.86, 0.86));
            x += text_width(area, state, plain);
        }
        let segment = &source[range_start..range_end];
        let segment_width = text_width(area, state, segment);
        if x <= max_x && x + segment_width >= min_x {
            draw_plain_text(
                area,
                context,
                state,
                segment,
                x,
                baseline,
                range.style.color(),
            );
        }
        x += segment_width;
        cursor = range_end;
    }
    if cursor < end {
        draw_plain_text(
            area,
            context,
            state,
            &source[cursor..end],
            x,
            baseline,
            (0.86, 0.86, 0.86),
        );
    }
}

fn valid_highlight_range(source: &str, range: &HighlightRange) -> bool {
    range.start < range.end
        && range.end <= source.len()
        && source.is_char_boundary(range.start)
        && source.is_char_boundary(range.end)
}

pub(super) fn draw_syntax_issues(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    source: &str,
    issues: &[SyntaxIssue],
    start: usize,
    end: usize,
    text_x: f64,
    baseline: f64,
    theme: EditorTheme,
) {
    if !state.editable.get() {
        return;
    }

    let first_issue = issues.partition_point(|issue| issue.end <= start);
    for issue in &issues[first_issue..] {
        if issue.start >= end {
            break;
        }
        if !valid_syntax_issue(source, issue) {
            continue;
        }
        let issue_start = issue.start.max(start);
        let issue_end = issue.end.min(end);
        if issue_start >= issue_end {
            continue;
        }
        let x = text_x + text_width(area, state, &source[start..issue_start]);
        let width = text_width(area, state, &source[issue_start..issue_end]);
        draw_wavy_underline(
            context,
            x,
            baseline + 2.0,
            width,
            theme.syntax_error_underline,
        );
    }
}

fn valid_syntax_issue(source: &str, issue: &SyntaxIssue) -> bool {
    issue.start < issue.end
        && issue.end <= source.len()
        && source.is_char_boundary(issue.start)
        && source.is_char_boundary(issue.end)
}

pub(super) fn draw_spellcheck_issues(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    source: &str,
    issues: &[SpellcheckIssue],
    start: usize,
    end: usize,
    text_x: f64,
    baseline: f64,
    theme: EditorTheme,
) {
    let first_issue = issues.partition_point(|issue| issue.end <= start);
    for issue in &issues[first_issue..] {
        if issue.start >= end {
            break;
        }
        if issue.start >= issue.end
            || issue.end > source.len()
            || !source.is_char_boundary(issue.start)
            || !source.is_char_boundary(issue.end)
        {
            continue;
        }
        let issue_start = issue.start.max(start);
        let issue_end = issue.end.min(end);
        if issue_start >= issue_end {
            continue;
        }
        let x = text_x + text_width(area, state, &source[start..issue_start]);
        let width = text_width(area, state, &source[issue_start..issue_end]);
        draw_wavy_underline(
            context,
            x,
            baseline + 2.0,
            width,
            theme.spellcheck_underline,
        );
    }
}

fn draw_wavy_underline(context: &cairo::Context, x: f64, y: f64, width: f64, color: Color) {
    if width <= 1.0 {
        return;
    }
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
    context.set_line_width(1.2);
    let step = 3.0;
    let amplitude = 1.4;
    context.move_to(x, y);
    let mut current = 0.0;
    let mut up = true;
    while current < width {
        current = (current + step).min(width);
        let next_y = if up { y - amplitude } else { y + amplitude };
        context.line_to(x + current, next_y);
        up = !up;
    }
    let _ = context.stroke();
}
