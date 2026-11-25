use craic_file_support::LanguageId;
use craic_language::language_support_for_id;
use craic_render_skia::{
    DragSelection, EditorDocument, EditorFoldRange, EditorMetrics, EditorSearchMatch,
    EditorSelection, EditorViewport, SelectionMode, TextDiagnosticSpan, TextSyntaxSpan,
    VERTICAL_SCROLLBAR_WIDTH, drag_for_mode, editor_search_index_after, find_editor_search_matches,
    next_char_boundary, next_word_boundary, previous_char_boundary, previous_word_boundary,
    selection_for_drag, toggle_editor_line_comment, word_bounds_at,
};
use regex::RegexBuilder;
use std::collections::VecDeque;
use std::ops::Range;

#[derive(Clone, Copy, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Copy)]
pub enum Motion {
    Character,
    Word,
    Row,
    LineEdge,
    DocumentEdge,
}

pub struct Editor {
    pub path: String,
    pub language: LanguageId,
    pub document: EditorDocument,
    pub selection: EditorSelection,
    pub viewport: EditorViewport,
    pub metrics: EditorMetrics,
    pub editable: bool,
    pub marked_text: String,
    pub diagnostics: Vec<TextDiagnosticSpan>,
    pub search_matches: Vec<EditorSearchMatch>,
    pub active_search_match: Option<usize>,
    pub manual_folds: Vec<(usize, usize)>,
    pub completion_items: Vec<String>,
    pub completion_selected: usize,
    pub completion_range: Option<(usize, usize)>,
    search_query: String,
    search_options: SearchOptions,
    undo: VecDeque<Edit>,
    redo: VecDeque<Edit>,
}

struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    before: EditorSelection,
    after: EditorSelection,
}

impl Editor {
    pub fn new(font_size: f32) -> Self {
        Self {
            path: String::new(),
            language: LanguageId::PlainText,
            document: EditorDocument::default(),
            selection: EditorSelection::default(),
            viewport: EditorViewport::default(),
            metrics: EditorMetrics::for_font_size(font_size),
            editable: false,
            marked_text: String::new(),
            diagnostics: Vec::new(),
            search_matches: Vec::new(),
            active_search_match: None,
            manual_folds: Vec::new(),
            completion_items: Vec::new(),
            completion_selected: 0,
            completion_range: None,
            search_query: String::new(),
            search_options: SearchOptions::default(),
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        }
    }

    pub fn reset_document(
        &mut self,
        text: String,
        syntax: Vec<TextSyntaxSpan>,
        selection: EditorSelection,
        reset_scroll: bool,
    ) {
        self.document = EditorDocument::new(text, syntax);
        self.marked_text.clear();
        self.diagnostics.clear();
        self.manual_folds.clear();
        self.clear_completion();
        self.clear_search();
        self.undo.clear();
        self.redo.clear();
        if reset_scroll {
            self.viewport.reset();
        }
        self.select(selection);
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        self.viewport.resize(
            width,
            height,
            &mut self.document,
            self.metrics,
            VERTICAL_SCROLLBAR_WIDTH,
        );
    }

    pub fn clamp_scroll(&mut self) {
        self.resize(self.viewport.width(), self.viewport.height());
    }

    pub fn reveal_selection(&mut self) {
        self.clamp_scroll();
        self.viewport.reveal_offset(
            &self.document,
            self.selection.focus,
            self.metrics,
            VERTICAL_SCROLLBAR_WIDTH,
        );
    }

    pub fn select(&mut self, selection: EditorSelection) {
        self.selection = EditorSelection {
            anchor: self.document.clamp_offset(selection.anchor),
            focus: self.document.clamp_offset(selection.focus),
        };
        self.reveal_selection();
    }

    pub fn hit_test(&self, x: f64, y: f64) -> usize {
        self.document.hit_test(
            x,
            y,
            self.viewport.scroll_x(),
            self.viewport.scroll_y(),
            self.metrics,
        )
    }

    pub fn start_drag(
        &mut self,
        x: f64,
        y: f64,
        presses: i32,
        extend: bool,
    ) -> DragSelection<usize> {
        self.clear_completion();
        let offset = self.hit_test(x, y);
        if extend {
            self.selection.focus = offset;
            return DragSelection::Character {
                anchor: self.selection.anchor,
            };
        }
        let (drag, selection) = drag_for_mode(
            offset,
            SelectionMode::for_press_count(presses),
            |offset| word_bounds_at(self.document.text(), offset),
            |offset| self.line_bounds(offset),
        );
        self.selection = EditorSelection {
            anchor: selection.anchor,
            focus: selection.focus,
        };
        drag
    }

    pub fn drag_to(&mut self, drag: DragSelection<usize>, x: f64, y: f64) -> bool {
        let scrolled = self
            .viewport
            .autoscroll_for_pointer(x, y, &self.document, self.metrics);
        let selection = selection_for_drag(
            drag,
            self.hit_test(x, y),
            |offset| word_bounds_at(self.document.text(), offset),
            |offset| self.line_bounds(offset),
        );
        self.selection = EditorSelection {
            anchor: selection.anchor,
            focus: selection.focus,
        };
        scrolled
    }

    fn line_bounds(&self, offset: usize) -> Option<(usize, usize)> {
        let (line, _) = self.document.line_column_for_offset(offset);
        self.document
            .lines()
            .get(line)
            .map(|line| (line.start, line.end_with_newline))
    }

    pub fn move_selection(&mut self, motion: Motion, forward: bool, extend: bool) {
        self.clear_completion();
        let focus = self.selection.focus;
        let text = self.document.text();
        let (start, end) = self.selection.normalized();
        let offset = match motion {
            Motion::Character | Motion::Word if !extend && start != end => {
                if forward {
                    end
                } else {
                    start
                }
            }
            Motion::Character if forward => next_char_boundary(text, focus),
            Motion::Character => previous_char_boundary(text, focus),
            Motion::Word if forward => next_word_boundary(text, focus),
            Motion::Word => previous_word_boundary(text, focus),
            Motion::Row => {
                let row = self.document.visual_line_for_offset(focus);
                let column = self.document.visual_column_for_offset(focus);
                let row = if forward {
                    (row + 1).min(self.document.visual_lines().len().saturating_sub(1))
                } else {
                    row.saturating_sub(1)
                };
                self.document.offset_for_visual_line_column(row, column)
            }
            Motion::LineEdge => {
                let (line, _) = self.document.line_column_for_offset(focus);
                self.document
                    .lines()
                    .get(line)
                    .map_or(0, |line| if forward { line.end } else { line.start })
            }
            Motion::DocumentEdge if forward => text.len(),
            Motion::DocumentEdge => 0,
        };
        self.select(EditorSelection {
            anchor: if extend {
                self.selection.anchor
            } else {
                offset
            },
            focus: offset,
        });
    }

    pub fn replace_text(&mut self, range: Option<Range<usize>>, replacement: &str) -> bool {
        let (start, end) = self.selection.normalized();
        let range = range.unwrap_or(start..end);
        let caret = range.start + replacement.len();
        self.edit(
            range,
            replacement,
            EditorSelection {
                anchor: caret,
                focus: caret,
            },
        )
    }

    pub fn delete(&mut self, forward: bool, word: bool) -> bool {
        let range = self.selection.is_empty().then(|| {
            let focus = self.selection.focus;
            let text = self.document.text();
            if forward {
                focus..if word {
                    next_word_boundary(text, focus)
                } else {
                    next_char_boundary(text, focus)
                }
            } else {
                let start = if word {
                    previous_word_boundary(text, focus)
                } else {
                    previous_char_boundary(text, focus)
                };
                start..focus
            }
        });
        self.replace_text(range, "")
    }

    pub fn toggle_line_comment(&mut self) -> bool {
        let Some(prefix) = language_support_for_id(self.language).line_comment else {
            return false;
        };
        let Some(edit) = toggle_editor_line_comment(self.document.text(), self.selection, prefix)
        else {
            return false;
        };
        self.edit(
            edit.range_start..edit.range_end,
            &edit.replacement,
            edit.map_selection(self.selection),
        )
    }

    fn edit(&mut self, range: Range<usize>, replacement: &str, selection: EditorSelection) -> bool {
        let Some(removed) = self.document.text().get(range.clone()) else {
            return false;
        };
        if !self.editable || (removed.is_empty() && replacement.is_empty()) {
            return false;
        }
        self.undo.push_back(Edit {
            start: range.start,
            removed: removed.to_owned(),
            inserted: replacement.to_owned(),
            before: self.selection,
            after: selection,
        });
        if self.undo.len() > 100 {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.apply_edit(range, replacement, selection);
        true
    }

    fn apply_edit(&mut self, range: Range<usize>, replacement: &str, selection: EditorSelection) {
        let mut text = self.document.text().to_owned();
        text.replace_range(range, replacement);
        self.document = EditorDocument::new(text, Vec::new());
        self.diagnostics.clear();
        self.manual_folds.clear();
        self.clear_completion();
        self.select(selection);
        // Invalid regular expressions keep the existing search panel error until corrected.
        let _ = self.rebuild_search();
    }

    pub fn undo(&mut self, redo: bool) -> bool {
        if !self.editable {
            return false;
        }
        if redo {
            let Some(edit) = self.redo.pop_back() else {
                return false;
            };
            self.apply_edit(
                edit.start..edit.start + edit.removed.len(),
                &edit.inserted,
                edit.after,
            );
            self.undo.push_back(edit);
        } else {
            let Some(edit) = self.undo.pop_back() else {
                return false;
            };
            self.apply_edit(
                edit.start..edit.start + edit.inserted.len(),
                &edit.removed,
                edit.before,
            );
            self.redo.push_back(edit);
        }
        true
    }

    pub fn set_syntax_and_folds(
        &mut self,
        syntax: Vec<TextSyntaxSpan>,
        ranges: Vec<(usize, usize)>,
    ) {
        let folds = ranges
            .into_iter()
            .chain(self.manual_folds.iter().copied())
            .map(|(start_line, end_line)| {
                let expanded = self
                    .document
                    .fold_starting_at(start_line)
                    .filter(|fold| fold.end_line == end_line)
                    .is_none_or(|fold| fold.expanded);
                EditorFoldRange {
                    start_line,
                    end_line,
                    expanded,
                }
            })
            .collect();
        self.document =
            EditorDocument::new_with_folds(self.document.text().to_owned(), syntax, folds);
        self.clamp_scroll();
    }

    pub fn toggle_fold(&mut self, y: f64) -> bool {
        let row = ((y + self.viewport.scroll_y() - f64::from(self.metrics.text_inset))
            / f64::from(self.metrics.line_height))
        .floor() as usize;
        let Some(line) = self.document.visual_lines().get(row) else {
            return false;
        };
        if line.wrap_index != 0 {
            return false;
        }
        let mut folds = self.document.folds().to_vec();
        let Some(fold) = folds
            .iter_mut()
            .find(|fold| fold.start_line == line.source_line)
        else {
            return false;
        };
        fold.expanded = !fold.expanded;
        if !fold.expanded
            && let Some(first_hidden) = self.document.lines().get(fold.start_line + 1)
            && let Some(last_hidden) = self.document.lines().get(fold.end_line)
            && (first_hidden.start..last_hidden.end_with_newline).contains(&self.selection.focus)
        {
            let caret = self.document.lines()[fold.start_line].end;
            self.selection = EditorSelection {
                anchor: caret,
                focus: caret,
            };
        }
        self.replace_folds(folds);
        true
    }

    pub fn replace_folds(&mut self, folds: Vec<EditorFoldRange>) {
        self.document = EditorDocument::new_with_folds(
            self.document.text().to_owned(),
            self.document.syntax().to_vec(),
            folds,
        );
        self.clamp_scroll();
    }

    pub fn set_diagnostics(&mut self, mut diagnostics: Vec<TextDiagnosticSpan>) {
        diagnostics.retain(|span| {
            span.start < span.end
                && span.end <= self.document.text().len()
                && self.document.text().is_char_boundary(span.start)
                && self.document.text().is_char_boundary(span.end)
        });
        diagnostics.sort_by_key(|span| (span.start, span.end));
        self.diagnostics = diagnostics;
    }

    pub fn set_search(&mut self, query: &str, options: SearchOptions) -> Result<(), regex::Error> {
        self.search_query = query.to_owned();
        self.search_options = options;
        self.rebuild_search()?;
        self.activate_match();
        Ok(())
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_matches.clear();
        self.active_search_match = None;
    }

    fn rebuild_search(&mut self) -> Result<(), regex::Error> {
        self.search_matches.clear();
        self.active_search_match = None;
        if self.search_query.is_empty() {
            return Ok(());
        }
        let options = self.search_options;
        let text = self.document.text();
        self.search_matches = if !options.case_sensitive
            && !options.whole_word
            && !options.regex
            && self.search_query.is_ascii()
        {
            find_editor_search_matches(text, &self.search_query)
        } else {
            let mut pattern = if options.regex {
                self.search_query.clone()
            } else {
                regex::escape(&self.search_query)
            };
            if options.whole_word {
                pattern = format!(r"\b(?:{pattern})\b");
            }
            RegexBuilder::new(&pattern)
                .case_insensitive(!options.case_sensitive)
                .build()?
                .find_iter(text)
                .map(|found| EditorSearchMatch {
                    start: found.start(),
                    end: found.end(),
                })
                .collect()
        };
        self.active_search_match =
            editor_search_index_after(&self.search_matches, self.selection.focus);
        Ok(())
    }

    pub fn step_search(&mut self, forward: bool) {
        let count = self.search_matches.len();
        if count == 0 {
            return;
        }
        self.active_search_match = Some(match (self.active_search_match, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => index.checked_sub(1).unwrap_or(count - 1),
            (None, true) => 0,
            (None, false) => count - 1,
        });
        self.activate_match();
    }

    fn activate_match(&mut self) {
        if let Some(matched) = self
            .active_search_match
            .and_then(|index| self.search_matches.get(index))
        {
            self.select(EditorSelection {
                anchor: matched.start,
                focus: matched.end,
            });
        }
    }

    pub fn step_completion(&mut self, forward: bool) -> bool {
        let count = self.completion_items.len();
        if count == 0 {
            return false;
        }
        self.completion_selected = if forward {
            (self.completion_selected + 1) % count
        } else {
            self.completion_selected.checked_sub(1).unwrap_or(count - 1)
        };
        true
    }

    pub fn accept_completion(&mut self) -> bool {
        let Some((from, to)) = self.completion_range else {
            return false;
        };
        let Some(item) = self.completion_items.get(self.completion_selected).cloned() else {
            return false;
        };
        self.replace_text(Some(from..to), &item)
    }

    pub fn clear_completion(&mut self) {
        self.completion_items.clear();
        self.completion_selected = 0;
        self.completion_range = None;
    }
}
