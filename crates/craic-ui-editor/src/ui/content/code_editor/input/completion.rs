fn clear_transient_selection(area: &gtk::GLArea, state: &Rc<EditorState>) -> bool {
    let cleared_selection = state.selection.borrow_mut().take().is_some();
    let cleared_preedit = {
        let mut preedit = state.preedit.borrow_mut();
        let cleared = !preedit.is_empty();
        if cleared {
            preedit.clear();
        }
        cleared
    };

    if !cleared_selection && !cleared_preedit {
        return false;
    }

    log::debug!(
        "code_editor escape cleared selection={cleared_selection} preedit={cleared_preedit}"
    );
    reset_cursor_blink(area, state);
    true
}

fn is_tab_key(key: gdk::Key) -> bool {
    matches!(key, gdk::Key::Tab | gdk::Key::ISO_Left_Tab)
}

fn edit_tab(area: &gtk::GLArea, state: &Rc<EditorState>, outdent: bool) {
    if outdent {
        edit_line_indentation(area, state, false);
    } else if selection_bounds(state).is_some() {
        edit_line_indentation(area, state, true);
    } else {
        insert_text(area, state, INDENT_TEXT);
    }
}

fn position_context_click(area: &gtk::GLArea, state: &Rc<EditorState>, x: f64, y: f64) {
    if render::fold_action_at_point(area, state, x, y).is_some() {
        return;
    }
    let offset = render::hit_test(area, state, x, y);
    let inside_selection =
        selection_bounds(state).is_some_and(|(start, end)| offset >= start && offset < end);
    if !inside_selection {
        move_cursor_to(area, state, offset, false);
    }
}

fn show_context_menu(area: &gtk::GLArea, state: &Rc<EditorState>, x: f64, y: f64) {
    let offset = render::hit_test(area, state, x, y);
    context_menu::popup_action_menu(area, x, y, editor_context_menu_sections(state, offset), {
        let area = area.clone();
        let state = state.clone();

        move |action| {
            run_action(&area, &state, action);
            area.grab_focus();
        }
    });
}

pub fn apply_completion_result(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    completions: Option<CompletionSet>,
) {
    let Some(completions) = completions.filter(|completions| !completions.items.is_empty()) else {
        clear_completion(state, false);
        return;
    };

    if completions.replacement_end != state.cursor.get() {
        clear_completion(state, false);
        return;
    }

    {
        let mut completion = state.completion.borrow_mut();
        completion.items = completions.items;
        completion.selected = 0;
        completion.replacement_range =
            Some((completions.replacement_start, completions.replacement_end));
    }

    show_completion_popover(area, state);
}

pub fn dismiss_completion(state: &Rc<EditorState>) {
    clear_completion(state, true);
}

fn handle_completion_key(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    key: gdk::Key,
    command: bool,
    shift: bool,
) -> bool {
    if !completion_is_open(state) || command {
        return false;
    }

    match key {
        gdk::Key::Escape => {
            dismiss_completion(state);
            true
        }
        gdk::Key::Down => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Up => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Left
        | gdk::Key::Right
        | gdk::Key::Home
        | gdk::Key::End
        | gdk::Key::Page_Up
        | gdk::Key::Page_Down => {
            dismiss_completion_for_navigation(state, key);
            false
        }
        gdk::Key::Return | gdk::Key::KP_Enter => {
            accept_completion(area, state);
            true
        }
        gdk::Key::Tab if !shift => {
            accept_completion(area, state);
            true
        }
        _ => false,
    }
}

fn dismiss_completion_for_navigation(state: &Rc<EditorState>, key: gdk::Key) {
    log::debug!("code_editor completion dismissed for navigation key={key:?}");
    dismiss_completion(state);
}

fn completion_is_open(state: &Rc<EditorState>) -> bool {
    let completion = state.completion.borrow();
    !completion.items.is_empty() && completion.replacement_range.is_some()
}

fn request_or_dismiss_completion(area: &gtk::GLArea, state: &Rc<EditorState>) {
    if !state.editable.get()
        || state.selection.borrow().is_some()
        || crate::language_support::language_support_for_id(state.language.get())
            .completion
            .is_none()
        || !text_has_completion_trigger(&state.text.borrow(), state.cursor.get())
    {
        dismiss_completion(state);
        return;
    }

    let request_id = next_completion_request_id(state);
    super::request_suggestions(
        state,
        request_id,
        state.cursor.get().min(state.text.borrow().len()),
    );
    position_completion_popover(area, state);
}

fn next_completion_request_id(state: &Rc<EditorState>) -> u64 {
    let mut completion = state.completion.borrow_mut();
    completion.request_id = completion.request_id.wrapping_add(1).max(1);
    completion.request_id
}

fn clear_completion(state: &Rc<EditorState>, invalidate_request: bool) {
    {
        let mut completion = state.completion.borrow_mut();
        if invalidate_request {
            completion.request_id = completion.request_id.wrapping_add(1).max(1);
        }
        completion.items.clear();
        completion.selected = 0;
        completion.replacement_range = None;
    }

    if let Some(ui) = state.completion_ui.borrow().as_ref() {
        ui.popover.popdown();
    }
}

fn show_completion_popover(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let ui = ensure_completion_ui(area, state);
    refill_completion_rows(&ui, state);
    select_completion_row(&ui, state.completion.borrow().selected);
    position_completion_popover(area, state);
    ui.popover.popup();
    area.grab_focus();
}

fn ensure_completion_ui(area: &gtk::GLArea, state: &Rc<EditorState>) -> CompletionUi {
    if let Some(ui) = state.completion_ui.borrow().as_ref().cloned() {
        return ui;
    }

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("code-editor-completion-list");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(220)
        .max_content_height(220)
        .propagate_natural_height(true)
        .child(&list)
        .build();

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(false)
        .position(gtk::PositionType::Bottom)
        .child(&scroller)
        .build();
    popover.add_css_class("menu");
    popover.add_css_class("code-editor-completion-popover");
    popover.set_halign(gtk::Align::Start);
    popover.set_parent(area);

    list.connect_row_activated({
        let area = area.clone();
        let state = state.clone();
        move |_, row| {
            let index = row.index();
            if index >= 0 {
                state.completion.borrow_mut().selected = index as usize;
            }
            accept_completion(&area, &state);
        }
    });

    let ui = CompletionUi { popover, list };
    state.completion_ui.replace(Some(ui.clone()));
    ui
}

fn refill_completion_rows(ui: &CompletionUi, state: &Rc<EditorState>) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }

    for item in state.completion.borrow().items.iter() {
        ui.list.append(&completion_row(&item.label));
    }
}

fn completion_row(label: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    label.add_css_class("code-editor-completion-label");

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    content.add_css_class("code-editor-completion-row");
    content.append(&label);

    gtk::ListBoxRow::builder()
        .selectable(true)
        .activatable(true)
        .child(&content)
        .build()
}

fn position_completion_popover(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let Some(ui) = state.completion_ui.borrow().as_ref().cloned() else {
        return;
    };
    let Some((x, y, width, height)) = render::cursor_rect(area, state) else {
        return;
    };

    ui.popover.set_pointing_to(Some(&gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        width.ceil().max(1.0) as i32,
        height.ceil().max(1.0) as i32,
    )));
}

fn select_completion_row(ui: &CompletionUi, selected: usize) {
    if let Some(row) = ui.list.row_at_index(selected as i32) {
        ui.list.select_row(Some(&row));
    }
}

fn accept_completion(area: &gtk::GLArea, state: &Rc<EditorState>) {
    let Some((item, start, end)) = ({
        let completion = state.completion.borrow();
        completion.replacement_range.and_then(|(start, end)| {
            completion
                .items
                .get(completion.selected)
                .cloned()
                .map(|item| (item, start, end))
        })
    }) else {
        dismiss_completion(state);
        return;
    };

    commit_edit(
        area,
        state,
        start,
        end,
        &item.insert_text,
        start + item.insert_text.len(),
        None,
        true,
    );
    dismiss_completion(state);
}

fn text_has_completion_trigger(text: &str, cursor: usize) -> bool {
    let cursor = previous_char_boundary(text, cursor.min(text.len()));
    let prefix_start = identifier_start_before(text, cursor);
    let Some(dot) = previous_non_whitespace(text, prefix_start) else {
        return false;
    };
    text[dot..].starts_with('.')
}

fn identifier_start_before(text: &str, cursor: usize) -> usize {
    let mut start = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, start) {
        if !(ch == '_' || ch.is_ascii_alphanumeric()) {
            break;
        }
        start = previous;
    }
    start
}

fn previous_non_whitespace(text: &str, cursor: usize) -> Option<usize> {
    let mut offset = cursor.min(text.len());
    while let Some((previous, ch)) = previous_char(text, offset) {
        if !ch.is_whitespace() {
            return Some(previous);
        }
        offset = previous;
    }
    None
}

fn editor_context_menu_sections(
    state: &Rc<EditorState>,
    offset: usize,
) -> Vec<context_menu::ActionMenuSection<TextContextAction>> {
    let mut sections = Vec::new();
    if let Some(section) = markdown_fix_section(state, offset) {
        sections.push(section);
    }
    if let Some(section) = spelling_correction_section(state, offset) {
        sections.push(section);
    }
    sections.extend(context_menu::text_context_menu_sections(
        TextContextMenuState {
            undo: MenuActionState::visible(action_enabled(state, TextContextAction::Undo)),
            redo: MenuActionState::visible(action_enabled(state, TextContextAction::Redo)),
            cut: MenuActionState::visible(action_enabled(state, TextContextAction::Cut)),
            copy: MenuActionState::visible(action_enabled(state, TextContextAction::Copy)),
            paste: MenuActionState::visible(action_enabled(state, TextContextAction::Paste)),
            select_all: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::SelectAll,
            )),
            fold_selection: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::FoldSelection,
            )),
            toggle_wrap: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::ToggleWrap,
            )),
            toggle_read_only: MenuActionState::visible(action_enabled(
                state,
                TextContextAction::ToggleReadOnly,
            )),
        },
    ));
    sections
}

fn markdown_fix_section(
    state: &Rc<EditorState>,
    offset: usize,
) -> Option<context_menu::ActionMenuSection<TextContextAction>> {
    if !state.editable.get() {
        return None;
    }
    let issue = markdown_lint_issue_at(state, offset)?;
    let rule_name = issue.rule_name.clone()?;
    let mut items = Vec::new();
    if let Some(fix) = issue.fix {
        items.push(context_menu::ActionMenuItem::with_icon(
            "Fix Markdown Issue",
            "lightbulb-symbolic",
            TextContextAction::ApplyMarkdownFix { edits: fix.edits },
            true,
        ));
    }
    items.push(context_menu::ActionMenuItem::with_icon(
        format!("Ignore {rule_name} in Repo Config"),
        "lightbulb-symbolic",
        TextContextAction::AddMarkdownLintIgnore { rule_name },
        true,
    ));
    Some(context_menu::ActionMenuSection::new(items))
}

fn spelling_correction_section(
    state: &Rc<EditorState>,
    offset: usize,
) -> Option<context_menu::ActionMenuSection<TextContextAction>> {
    if !state.editable.get() {
        return None;
    }
    let issue = spellcheck_issue_at(state, offset)?;
    let items = issue
        .corrections
        .iter()
        .take(5)
        .map(|correction| {
            context_menu::ActionMenuItem::new(
                format!("Replace with \"{correction}\""),
                TextContextAction::CorrectSpelling {
                    start: issue.start,
                    end: issue.end,
                    replacement: correction.clone(),
                },
                true,
            )
        })
        .collect::<Vec<_>>();
    (!items.is_empty()).then(|| context_menu::ActionMenuSection::new(items))
}

fn spellcheck_issue_at(state: &Rc<EditorState>, offset: usize) -> Option<SpellcheckIssue> {
    state
        .spellcheck_issues
        .borrow()
        .iter()
        .find(|issue| issue.start <= offset && offset <= issue.end)
        .cloned()
}

fn markdown_lint_issue_at(state: &Rc<EditorState>, offset: usize) -> Option<MarkdownLintIssue> {
    state
        .markdown_lint_issues
        .borrow()
        .iter()
        .find(|issue| issue.start <= offset && offset <= issue.end && issue.rule_name.is_some())
        .cloned()
}

fn action_enabled(state: &Rc<EditorState>, action: TextContextAction) -> bool {
    match action {
        TextContextAction::ApplyMarkdownFix { edits } => state.editable.get() && !edits.is_empty(),
        TextContextAction::AddMarkdownLintIgnore { .. } => true,
        TextContextAction::CorrectSpelling { .. } => state.editable.get(),
        TextContextAction::Undo => state.editable.get() && !state.undo_stack.borrow().is_empty(),
        TextContextAction::Redo => state.editable.get() && !state.redo_stack.borrow().is_empty(),
        TextContextAction::Copy => selection_bounds(state).is_some(),
        TextContextAction::Cut => state.editable.get() && selection_bounds(state).is_some(),
        TextContextAction::Paste => state.editable.get(),
        TextContextAction::SelectAll => !state.text.borrow().is_empty(),
        TextContextAction::FoldSelection => state.editable.get() && selection_spans_lines(state),
        TextContextAction::ToggleWrap => true,
        TextContextAction::ToggleReadOnly => true,
    }
}

fn run_action(area: &gtk::GLArea, state: &Rc<EditorState>, action: TextContextAction) {
    match action {
        TextContextAction::ApplyMarkdownFix { edits } => {
            apply_markdown_fix(area, state, &edits);
        }
        TextContextAction::AddMarkdownLintIgnore { rule_name } => {
            for callback in state.markdown_lint_ignore_callbacks.borrow().iter() {
                callback(rule_name.clone());
            }
        }
        TextContextAction::CorrectSpelling {
            start,
            end,
            replacement,
        } => {
            let cursor = start + replacement.len();
            commit_edit(area, state, start, end, &replacement, cursor, None, true);
        }
        TextContextAction::Undo => undo(area, state),
        TextContextAction::Redo => redo(area, state),
        TextContextAction::Copy => copy_selection(state),
        TextContextAction::Cut => {
            if state.editable.get() && selection_bounds(state).is_some() {
                copy_selection(state);
                delete_selection(area, state);
            }
        }
        TextContextAction::Paste => paste_from_clipboard(area, state),
        TextContextAction::SelectAll => {
            state.selection.replace(Some(Selection {
                anchor: 0,
                focus: state.text.borrow().len(),
                visual_anchor: 0,
                visual_focus: state.text.borrow().len(),
            }));
            state.cursor.set(state.text.borrow().len());
            area.queue_render();
        }
        TextContextAction::ToggleWrap => {
            state.wrap.set(!state.wrap.get());
            render::invalidate_layout(state);
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
            area.queue_render();
        }
        TextContextAction::ToggleReadOnly => {
            state.editable.set(!state.editable.get());
            area.set_focusable(true);
            area.set_cursor_from_name(None);
            state.cursor_visible.set(true);
            area.queue_render();
        }
        TextContextAction::FoldSelection => fold_selection(area, state),
    }
}

fn apply_markdown_fix(area: &gtk::GLArea, state: &Rc<EditorState>, edits: &[MarkdownLintEdit]) {
    if edits.is_empty() {
        return;
    }

    let fixed = {
        let text = state.text.borrow();
        let Some(fixed) = apply_markdown_edits(text.as_str(), edits) else {
            log::warn!("markdown fix skipped reason=invalid-edit");
            return;
        };
        if fixed == text.as_str() {
            return;
        }
        fixed
    };

    let cursor = state.cursor.get().min(fixed.len());
    let old_end = state.text.borrow().len();
    commit_edit(area, state, 0, old_end, &fixed, cursor, None, true);
}

fn apply_markdown_edits(source: &str, edits: &[MarkdownLintEdit]) -> Option<String> {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|edit| (edit.start, edit.end));
    for window in edits.windows(2) {
        if window[0].end > window[1].start {
            return None;
        }
    }

    let mut fixed = source.to_string();
    for edit in edits.iter().rev() {
        if edit.start > edit.end
            || edit.end > fixed.len()
            || !fixed.is_char_boundary(edit.start)
            || !fixed.is_char_boundary(edit.end)
        {
            return None;
        }
        fixed.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Some(fixed)
}
