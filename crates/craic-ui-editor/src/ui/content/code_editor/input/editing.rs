fn copy_selection(state: &Rc<EditorState>) {
    let Some((start, end)) = selection_bounds(state) else {
        return;
    };
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let text = state.text.borrow();
    display.clipboard().set_text(&text[start..end]);
}

fn paste_from_clipboard(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let area = area.clone();
    let state = state.clone();
    gtk::glib::MainContext::default().spawn_local(async move {
        let Ok(Some(text)) = display.clipboard().read_text_future().await else {
            return;
        };
        if !state.editable.get() {
            return;
        }
        insert_text(&area, &state, &text);
    });
}

fn insert_text(area: &gtk::GLArea, state: &Rc<EditorState>, inserted: &str) {
    let text = state.text.borrow();
    let (start, end) = selection_bounds(state).unwrap_or_else(|| {
        let cursor = state.cursor.get().min(text.len());
        (cursor, cursor)
    });
    drop(text);
    let cursor = start + inserted.len();
    commit_edit(area, state, start, end, inserted, cursor, None, true);
}

fn matching_auto_pair(ch: char) -> Option<char> {
    match ch {
        '(' => Some(')'),
        '<' => Some('>'),
        _ => None,
    }
}

fn insert_auto_pair(area: &gtk::GLArea, state: &Rc<EditorState>, open: char, close: char) {
    let text = state.text.borrow();
    let (start, end) = selection_bounds(state).unwrap_or_else(|| {
        let cursor = state.cursor.get().min(text.len());
        (cursor, cursor)
    });
    let selected = text[start..end].to_string();
    drop(text);

    let mut inserted = String::with_capacity(open.len_utf8() + selected.len() + close.len_utf8());
    inserted.push(open);
    inserted.push_str(&selected);
    inserted.push(close);

    let inner_start = start + open.len_utf8();
    let inner_end = inner_start + selected.len();
    let restored_selection = (start < end).then_some(Selection {
        anchor: inner_start,
        focus: inner_end,
        visual_anchor: inner_start,
        visual_focus: inner_end,
    });
    commit_edit(
        area,
        state,
        start,
        end,
        &inserted,
        inner_end,
        restored_selection,
        true,
    );
}

fn insert_newline(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let text = state.text.borrow();
    let (start, end) = selection_bounds(state).unwrap_or_else(|| {
        let cursor = state.cursor.get().min(text.len());
        (cursor, cursor)
    });
    let language = crate::language_support::language_support_for_id(state.language.get());
    let newline = enter_newline(NewlineContext {
        language,
        text: &text,
        cursor: start,
    });
    drop(text);

    commit_edit(
        area,
        state,
        start,
        end,
        &newline.inserted,
        newline.cursor,
        None,
        true,
    );
}

#[derive(Clone, Copy)]
struct LinePrefixEdit {
    start: usize,
    removed: usize,
    inserted: usize,
}

fn edit_line_indentation(area: &gtk::GLArea, state: &Rc<EditorState>, indent: bool) {
    let text = state.text.borrow();
    let (range_start, range_end, replacement, edits) = line_indentation_edit(&text, state, indent);
    let before_cursor = state.cursor.get().min(text.len());
    let before_selection = *state.selection.borrow();
    drop(text);

    if edits.is_empty() {
        log::debug!(
            "code_editor indentation skipped action={}",
            if indent { "indent" } else { "outdent" }
        );
        return;
    }

    let cursor = map_offset_through_prefix_edits(before_cursor, &edits);
    let restored_selection = before_selection.map(|selection| Selection {
        anchor: map_offset_through_prefix_edits(selection.anchor, &edits),
        focus: map_offset_through_prefix_edits(selection.focus, &edits),
        visual_anchor: map_offset_through_prefix_edits(selection.visual_anchor, &edits),
        visual_focus: map_offset_through_prefix_edits(selection.visual_focus, &edits),
    });
    log::debug!(
        "code_editor indentation action={} range={}..{} lines={}",
        if indent { "indent" } else { "outdent" },
        range_start,
        range_end,
        edits.len()
    );
    commit_edit(
        area,
        state,
        range_start,
        range_end,
        &replacement,
        cursor,
        restored_selection,
        true,
    );
}

fn line_indentation_edit(
    text: &str,
    state: &Rc<EditorState>,
    indent: bool,
) -> (usize, usize, String, Vec<LinePrefixEdit>) {
    let (first_line_start, last_line_start) = indentation_line_range(text, state);
    let range_start = first_line_start;
    let range_end = current_line_end(text, last_line_start);
    let line_starts = line_starts_between(text, first_line_start, last_line_start);

    let mut replacement = String::with_capacity(
        range_end
            .saturating_sub(range_start)
            .saturating_add(line_starts.len() * INDENT_TEXT.len()),
    );
    let mut edits = Vec::new();
    let mut copied_until = range_start;

    for line_start in line_starts {
        replacement.push_str(&text[copied_until..line_start]);
        if indent {
            replacement.push_str(INDENT_TEXT);
            edits.push(LinePrefixEdit {
                start: line_start,
                removed: 0,
                inserted: INDENT_TEXT.len(),
            });
            copied_until = line_start;
            continue;
        }

        let line_end = current_line_end(text, line_start);
        let removed = outdent_len(&text[line_start..line_end]);
        if removed == 0 {
            copied_until = line_start;
            continue;
        }

        edits.push(LinePrefixEdit {
            start: line_start,
            removed,
            inserted: 0,
        });
        copied_until = line_start + removed;
    }

    replacement.push_str(&text[copied_until..range_end]);
    (range_start, range_end, replacement, edits)
}

fn toggle_line_comment(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }

    let language = crate::language_support::language_support_for_id(state.language.get());
    let Some(prefix) = language.line_comment else {
        log::debug!(
            "code_editor line_comment skipped unsupported language={:?}",
            language.id
        );
        return;
    };

    let text = state.text.borrow();
    let before_cursor = state.cursor.get().min(text.len());
    let before_selection = *state.selection.borrow();
    let selection = before_selection.map_or(
        SharedEditorSelection {
            anchor: before_cursor,
            focus: before_cursor,
        },
        |selection| SharedEditorSelection {
            anchor: selection.anchor,
            focus: selection.focus,
        },
    );
    let Some(edit) = toggle_editor_line_comment(&text, selection, prefix) else {
        log::debug!("code_editor line_comment skipped no applicable lines");
        return;
    };
    drop(text);

    let cursor = edit.map_offset(before_cursor);
    let restored_selection = before_selection.map(|selection| Selection {
        anchor: edit.map_offset(selection.anchor),
        focus: edit.map_offset(selection.focus),
        visual_anchor: edit.map_offset(selection.visual_anchor),
        visual_focus: edit.map_offset(selection.visual_focus),
    });
    log::debug!(
        "code_editor line_comment action={} range={}..{} lines={}",
        if edit.uncomment {
            "uncomment"
        } else {
            "comment"
        },
        edit.range_start,
        edit.range_end,
        edit.line_count()
    );
    commit_edit(
        area,
        state,
        edit.range_start,
        edit.range_end,
        &edit.replacement,
        cursor,
        restored_selection,
        true,
    );
}

fn indentation_line_range(text: &str, state: &Rc<EditorState>) -> (usize, usize) {
    let cursor = state.cursor.get().min(text.len());
    let (start, end) = selection_bounds(state).unwrap_or((cursor, cursor));
    let first_line_start = current_line_start(text, start);
    let last_offset = if end > start {
        previous_grapheme_offset(text, end)
    } else {
        start
    };
    let last_line_start = current_line_start(text, last_offset);
    (first_line_start, last_line_start)
}

fn line_starts_between(text: &str, first_line_start: usize, last_line_start: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut line_start = first_line_start.min(text.len());
    loop {
        starts.push(line_start);
        if line_start >= last_line_start {
            break;
        }

        let line_end = current_line_end(text, line_start);
        if line_end >= text.len() {
            break;
        }
        line_start = (line_end + 1).min(text.len());
    }
    starts
}

fn outdent_len(line: &str) -> usize {
    if line.starts_with('\t') {
        return '\t'.len_utf8();
    }

    line.as_bytes()
        .iter()
        .take(INDENT_TEXT.len())
        .take_while(|byte| **byte == b' ')
        .count()
}

fn map_offset_through_prefix_edits(offset: usize, edits: &[LinePrefixEdit]) -> usize {
    let mut adjustment = 0isize;

    for edit in edits {
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

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .take_while(|(_, ch)| matches!(ch, ' ' | '\t'))
        .map(|(offset, ch)| offset + ch.len_utf8())
        .last()
        .unwrap_or(0);
    &line[..end]
}

fn edit_backspace(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if delete_selection(area, state) {
        return;
    }
    let (start, cursor) = {
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        (previous_grapheme_offset(&text, cursor), cursor)
    };
    if start == cursor {
        return;
    }
    commit_edit(area, state, start, cursor, "", start, None, true);
}

fn edit_delete(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if delete_selection(area, state) {
        return;
    }
    let Some((cursor, end)) = ({
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        (cursor < text.len()).then(|| (cursor, next_grapheme_offset(&text, cursor)))
    }) else {
        return;
    };
    commit_edit(area, state, cursor, end, "", cursor, None, true);
}

fn edit_delete_leading_whitespace(area: &gtk::GLArea, state: &Rc<EditorState>) -> bool {
    if selection_bounds(state).is_some() {
        return false;
    }

    let Some((start, end)) = ({
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        let line_start = current_line_start(&text, cursor);
        let line_end = current_line_end(&text, line_start);
        let indent_end = line_start + leading_whitespace(&text[line_start..line_end]).len();
        (line_start < indent_end && cursor <= indent_end).then_some((line_start, indent_end))
    }) else {
        return false;
    };

    commit_edit(area, state, start, end, "", start, None, true);
    true
}

#[derive(Clone, Copy)]
enum DeleteDirection {
    Backward,
    Forward,
}

fn edit_delete_word(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    direction: DeleteDirection,
    extend_selection: bool,
) {
    if extend_selection {
        delete_word_range(area, state, direction);
        return;
    }

    if delete_selection(area, state) {
        return;
    }

    let (cursor, target) = {
        let text = state.text.borrow();
        let cursor = state.cursor.get().min(text.len());
        let target = match direction {
            DeleteDirection::Backward => previous_word_start(&text, cursor),
            DeleteDirection::Forward => next_word_end(&text, cursor),
        };
        (cursor, target)
    };
    let (start, end) = ordered_offsets(cursor, target);
    if start == end {
        return;
    }

    commit_edit(area, state, start, end, "", start, None, true);
}

fn delete_word_range(area: &gtk::GLArea, state: &Rc<EditorState>, direction: DeleteDirection) {
    let text = state.text.borrow();
    let len = text.len();
    let selection = *state.selection.borrow();
    let anchor = selection
        .map(|selection| selection.anchor)
        .unwrap_or_else(|| state.cursor.get().min(len));
    let focus = selection
        .map(|selection| selection.focus)
        .unwrap_or_else(|| state.cursor.get().min(len));
    let target = match direction {
        DeleteDirection::Backward => previous_word_start(&text, focus),
        DeleteDirection::Forward => next_word_end(&text, focus),
    };
    let (start, end) = ordered_offsets(anchor, target);
    if start == end {
        drop(text);
        delete_selection(area, state);
        return;
    }

    drop(text);
    commit_edit(area, state, start, end, "", start, None, true);
}

fn delete_selection(area: &gtk::GLArea, state: &Rc<EditorState>) -> bool {
    let Some((start, end)) = selection_bounds(state) else {
        return false;
    };
    commit_edit(area, state, start, end, "", start, None, true);
    true
}

fn move_cursor_to(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    offset: usize,
    extend_selection: bool,
) {
    dismiss_completion(state);
    let text_len = state.text.borrow().len();
    let offset = offset.min(text_len);
    if extend_selection {
        let anchor = state
            .selection
            .borrow()
            .map(|selection| selection.anchor)
            .unwrap_or_else(|| state.cursor.get().min(text_len));
        state.selection.replace(Some(Selection {
            anchor,
            focus: offset,
            visual_anchor: anchor,
            visual_focus: offset,
        }));
    } else {
        state.selection.borrow_mut().take();
    }
    state.cursor.set(offset);
    render::ensure_offset_visible(area, state, offset);
    reset_cursor_blink(area, state);
}

fn move_cursor_vertical(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    delta: isize,
    extend_selection: bool,
) {
    let Some(target) = render::vertical_cursor_target(area, state, delta) else {
        if !extend_selection && state.selection.borrow().is_some() {
            move_cursor_to(area, state, state.cursor.get(), false);
        } else {
            dismiss_completion(state);
        }
        return;
    };

    move_cursor_to(area, state, target, extend_selection);
}

fn cursor_left_target(state: &Rc<EditorState>, by_word: bool, extend_selection: bool) -> usize {
    if !extend_selection {
        if let Some((start, _)) = selection_bounds(state) {
            return start;
        }
    }

    let text = state.text.borrow();
    let cursor = state.cursor.get().min(text.len());
    if by_word {
        previous_word_start(&text, cursor)
    } else {
        previous_grapheme_offset(&text, cursor)
    }
}

fn cursor_right_target(state: &Rc<EditorState>, by_word: bool, extend_selection: bool) -> usize {
    if !extend_selection {
        if let Some((_, end)) = selection_bounds(state) {
            return end;
        }
    }

    let text = state.text.borrow();
    let cursor = state.cursor.get().min(text.len());
    if by_word {
        next_word_start(&text, cursor)
    } else {
        next_grapheme_offset(&text, cursor)
    }
}

fn scroll_page(area: &gtk::GLArea, state: &Rc<EditorState>, direction: isize) {
    let line_height = render::line_height(state);
    let distance = (area.allocated_height() as f64 - line_height).max(line_height);
    render::set_scroll_y(
        area,
        state,
        state.scroll_y.get() + distance * direction as f64,
    );
}

fn page_line_delta(area: &gtk::GLArea, state: &Rc<EditorState>, direction: isize) -> isize {
    let line_height = render::line_height(state);
    let lines = ((area.allocated_height() as f64 - line_height).max(line_height) / line_height)
        .floor()
        .max(1.0) as isize;
    lines * direction
}

fn select_at_mode(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    offset: usize,
    mode: SelectionMode,
) -> bool {
    let selection = selection_for_mode(
        offset,
        mode,
        |offset| word_drag_bounds(state, offset),
        |offset| line_drag_bounds(state, offset),
    );
    if selection.anchor == selection.focus {
        return false;
    }
    let raw_anchor = offset.min(state.text.borrow().len());
    state.selection.replace(Some(Selection {
        anchor: raw_anchor,
        focus: raw_anchor,
        visual_anchor: selection.anchor,
        visual_focus: selection.focus,
    }));
    state.cursor.set(selection.focus);
    render::ensure_offset_visible(area, state, selection.focus);
    reset_cursor_blink(area, state);
    true
}

fn selection_spans_lines(state: &Rc<EditorState>) -> bool {
    let Some((start, end)) = selection_bounds(state) else {
        return false;
    };
    let text = state.text.borrow();
    render::line_for_offset(&text, start) < render::line_for_offset(&text, end)
}

fn previous_word_start(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = previous;
    }

    let Some((_, ch)) = previous_char(text, offset) else {
        return offset;
    };
    let group = text_group(ch);
    while let Some((previous, ch)) = previous_char(text, offset) {
        if ch.is_whitespace() || text_group(ch) != group {
            break;
        }
        offset = previous;
    }

    offset
}

fn next_word_start(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    if let Some((_, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            let group = text_group(ch);
            while let Some((current, ch)) = next_char(text, offset) {
                if ch.is_whitespace() || text_group(ch) != group {
                    break;
                }
                offset = current + ch.len_utf8();
            }
        }
    }

    while let Some((current, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = current + ch.len_utf8();
    }

    offset
}

fn next_word_end(text: &str, cursor: usize) -> usize {
    let mut offset = cursor.min(text.len());
    while let Some((current, ch)) = next_char(text, offset) {
        if !ch.is_whitespace() {
            break;
        }
        offset = current + ch.len_utf8();
    }

    let Some((_, ch)) = next_char(text, offset) else {
        return offset;
    };
    let group = text_group(ch);
    while let Some((current, ch)) = next_char(text, offset) {
        if ch.is_whitespace() || text_group(ch) != group {
            break;
        }
        offset = current + ch.len_utf8();
    }

    offset
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextGroup {
    Word,
    Punctuation,
}

fn text_group(ch: char) -> TextGroup {
    if ch == '_' || ch.is_alphanumeric() {
        TextGroup::Word
    } else {
        TextGroup::Punctuation
    }
}

fn previous_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    text[..cursor.min(text.len())].char_indices().last()
}

fn next_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .char_indices()
        .next()
        .map(|(offset, ch)| (cursor + offset, ch))
}

fn ordered_offsets(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

fn previous_grapheme_offset(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .grapheme_indices(true)
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

fn next_grapheme_offset(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .grapheme_indices(true)
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn current_line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .rfind('\n')
        .map(|offset| offset + 1)
        .unwrap_or(0)
}

fn smart_home_target(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    let line_start = current_line_start(text, cursor);
    let line_end = current_line_end(text, line_start);
    let indent_end = line_start + leading_whitespace(&text[line_start..line_end]).len();

    if indent_end > line_start && cursor != indent_end {
        indent_end
    } else {
        line_start
    }
}

fn current_line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn logical_line_bounds_at(text: &str, offset: usize) -> (usize, usize) {
    let start = current_line_start(text, offset);
    let line_end = current_line_end(text, offset);
    let end = if line_end < text.len() {
        next_grapheme_offset(text, line_end)
    } else {
        line_end
    };
    (start, end)
}

fn commit_edit(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    start: usize,
    old_end: usize,
    replacement: &str,
    cursor: usize,
    restored_selection: Option<Selection>,
    record_history: bool,
) {
    let before_cursor = state.cursor.get();
    let before_selection = *state.selection.borrow();
    let (start, old_end, removed, text_changed, folds_may_change) = {
        let text = state.text.borrow();
        let start = previous_char_boundary(&text, start.min(text.len()));
        let old_end = previous_char_boundary(&text, old_end.min(text.len()).max(start));
        let removed = text[start..old_end].to_string();
        let text_changed = removed != replacement;
        let folds_may_change = text_affects_folds(&removed) || text_affects_folds(replacement);
        (start, old_end, removed, text_changed, folds_may_change)
    };
    if text_changed && record_history {
        push_history_snapshot(
            &mut state.undo_stack.borrow_mut(),
            HistorySnapshot {
                start,
                removed: removed.clone(),
                inserted: replacement.to_string(),
                before_cursor,
                before_selection,
                after_cursor: cursor,
                after_selection: restored_selection,
            },
        );
        state.redo_stack.borrow_mut().clear();
    }

    if text_changed {
        state
            .text
            .borrow_mut()
            .replace_range(start, old_end, replacement);
        state
            .document_revision
            .set(state.document_revision.get().wrapping_add(1).max(1));
        super::send_syntax_edit(
            state,
            start,
            old_end,
            replacement,
            folds_may_change && state.auto_folding_enabled.get(),
        );
    }
    state.cursor.set(cursor);
    state.selection.replace(restored_selection);
    if text_changed {
        render::invalidate_layout(state);
        render::invalidate_highlights(state);
        super::clear_git_state_for_state(state);
    }
    super::normalize_folds_for_current_text(state, "edit");
    if text_changed {
        notify_edit(state);
    }
    render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
    render::ensure_offset_visible(area, state, cursor);
    reset_cursor_blink(area, state);
}

fn undo(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(snapshot) = state.undo_stack.borrow_mut().pop() else {
        return;
    };
    restore_snapshot(area, state, &snapshot, HistoryDirection::Undo);
    push_history_snapshot(&mut state.redo_stack.borrow_mut(), snapshot);
}

fn redo(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if !state.editable.get() {
        return;
    }
    let Some(snapshot) = state.redo_stack.borrow_mut().pop() else {
        return;
    };
    restore_snapshot(area, state, &snapshot, HistoryDirection::Redo);
    push_history_snapshot(&mut state.undo_stack.borrow_mut(), snapshot);
}

fn push_history_snapshot(stack: &mut Vec<HistorySnapshot>, snapshot: HistorySnapshot) {
    if stack.len() >= MAX_HISTORY_SNAPSHOTS {
        stack.remove(0);
    }
    stack.push(snapshot);
}

#[derive(Clone, Copy)]
enum HistoryDirection {
    Undo,
    Redo,
}

fn restore_snapshot(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    snapshot: &HistorySnapshot,
    direction: HistoryDirection,
) {
    match direction {
        HistoryDirection::Undo => commit_edit(
            area,
            state,
            snapshot.start,
            snapshot.start + snapshot.inserted.len(),
            &snapshot.removed,
            snapshot.before_cursor,
            snapshot.before_selection,
            false,
        ),
        HistoryDirection::Redo => commit_edit(
            area,
            state,
            snapshot.start,
            snapshot.start + snapshot.removed.len(),
            &snapshot.inserted,
            snapshot.after_cursor,
            snapshot.after_selection,
            false,
        ),
    }
}

fn previous_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn text_affects_folds(text: &str) -> bool {
    text.bytes()
        .any(|byte| matches!(byte, b'\n' | b'{' | b'}' | b'[' | b']' | b'(' | b')' | b':'))
}

fn fold_selection(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let Some((start, end)) = selection_bounds(state) else {
        return;
    };
    let text = state.text.borrow();
    let start_line = render::line_for_offset(&text, start);
    let end_line = render::line_for_offset(&text, end);
    if end_line <= start_line {
        return;
    }
    state.folds.borrow_mut().push(FoldRange {
        start_line,
        end_line,
        expanded: false,
        automatic: false,
    });
    if !super::normalize_folds_for_current_text(state, "selection fold") {
        super::mark_fold_state_changed(state);
    }
    state.selection.borrow_mut().take();
    render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
    area.queue_render();
}

fn toggle_fold_at(area: &gtk::GLArea, state: &Rc<EditorState>, x: f64, y: f64) -> bool {
    let Some(action) = render::fold_action_at_point(area, state, x, y) else {
        return false;
    };
    match action {
        render::FoldAction::Toggle {
            index,
            start_line,
            end_line,
        } => {
            let toggled = {
                let mut folds = state.folds.borrow_mut();
                if let Some(fold) = folds
                    .get_mut(index)
                    .filter(|fold| fold.start_line == start_line && fold.end_line == end_line)
                {
                    fold.expanded = !fold.expanded;
                    true
                } else {
                    false
                }
            };
            if !toggled {
                log::debug!(
                    "code_editor fold toggle ignored stale index={index} range={start_line}..{end_line}"
                );
                return true;
            }
            super::mark_fold_state_changed(state);
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
            area.queue_render();
        }
    }
    true
}
