use super::*;
use crate::markdown_lint::MarkdownLintEdit;
use gtk::gio;

pub(super) fn install(
    view: &sourceview5::View,
    buffer: &sourceview5::Buffer,
    state: &Rc<EditorState>,
) {
    for (name, color) in [
        ("spellcheck", gdk::RGBA::new(0.9, 0.65, 0.1, 1.0)),
        ("markdown-lint", gdk::RGBA::new(0.9, 0.2, 0.2, 1.0)),
    ] {
        let tag = gtk::TextTag::builder()
            .name(name)
            .underline(gtk::pango::Underline::Error)
            .underline_rgba(&color)
            .build();
        buffer.tag_table().add(&tag);
    }
    // Let TextView provide its clipboard/undo menu, adding only diagnostic actions.
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let view = view.downgrade();
        let state = Rc::downgrade(state);
        move |_, _, x, y| {
            let (Some(view), Some(state)) = (view.upgrade(), state.upgrade()) else {
                return;
            };
            let (x, y) =
                view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = view.iter_at_location(x, y) {
                update_menu(&view, &state, iter.offset());
            }
        }
    });
    view.add_controller(click);
    buffer.connect_mark_set({
        let view = view.downgrade();
        let state = Rc::downgrade(state);
        move |_, iter, mark| {
            if mark.name().as_deref() == Some("insert") {
                if let (Some(view), Some(state)) = (view.upgrade(), state.upgrade()) {
                    update_menu(&view, &state, iter.offset());
                }
            }
        }
    });
    view.set_has_tooltip(true);
    view.connect_query_tooltip({
        let state = Rc::downgrade(state);
        move |view, x, y, keyboard, tooltip| {
            let Some(state) = state.upgrade() else {
                return false;
            };
            let buffer = view.buffer();
            let offset = if keyboard {
                buffer.cursor_position()
            } else {
                let (x, y) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x, y);
                let Some(iter) = view.iter_at_location(x, y) else {
                    return false;
                };
                iter.offset()
            };
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
            let offset = start_offset(&text, offset);
            let mut messages = Vec::new();
            for issue in state
                .spellcheck_issues
                .borrow()
                .iter()
                .filter(|issue| issue.start <= offset && offset < issue.end)
            {
                messages.push(format!("Spelling: {}", issue.word));
            }
            for issue in state
                .markdown_lint_issues
                .borrow()
                .iter()
                .filter(|issue| issue.start <= offset && offset < issue.end)
            {
                messages.push(format!(
                    "Markdown: {}",
                    issue.rule_name.as_deref().unwrap_or("Lint issue")
                ));
            }
            if messages.is_empty() {
                return false;
            }
            tooltip.set_text(Some(&messages.join("\n")));
            true
        }
    });
}

pub(super) fn clear(buffer: &sourceview5::Buffer, state: &EditorState) {
    state.spellcheck_issues.borrow_mut().clear();
    state.markdown_lint_issues.borrow_mut().clear();
    let (start, end) = buffer.bounds();
    for name in ["spellcheck", "markdown-lint"] {
        if let Some(tag) = buffer.tag_table().lookup(name) {
            buffer.remove_tag(&tag, &start, &end);
        }
    }
}

pub(super) fn apply_ranges(
    buffer: &sourceview5::Buffer,
    name: &str,
    ranges: impl Iterator<Item = (usize, usize)>,
) {
    let Some(tag) = buffer.tag_table().lookup(name) else {
        return;
    };
    let (start, end) = buffer.bounds();
    buffer.remove_tag(&tag, &start, &end);
    let text = buffer.text(&start, &end, true);
    for (start, end) in ranges {
        buffer.apply_tag(
            &tag,
            &iter_at_byte(buffer, &text, start),
            &iter_at_byte(buffer, &text, end),
        );
    }
}

pub(super) fn update_menu(view: &sourceview5::View, state: &Rc<EditorState>, char_offset: i32) {
    if !view.is_editable()
        || (state.spellcheck_issues.borrow().is_empty()
            && state.markdown_lint_issues.borrow().is_empty())
    {
        view.set_extra_menu(None::<&gio::Menu>);
        view.insert_action_group("diagnostics", None::<&gio::SimpleActionGroup>);
        return;
    }
    let buffer = view.buffer();
    let offset = buffer
        .text(
            &buffer.start_iter(),
            &buffer.iter_at_offset(char_offset),
            true,
        )
        .len();
    let menu = gio::Menu::new();
    let group = gio::SimpleActionGroup::new();
    let mut fixes = Vec::new();
    if view.is_editable() {
        for issue in state
            .spellcheck_issues
            .borrow()
            .iter()
            .filter(|issue| issue.start <= offset && offset <= issue.end)
        {
            for correction in &issue.corrections {
                fixes.push((
                    format!("Change to “{correction}”"),
                    vec![MarkdownLintEdit {
                        start: issue.start,
                        end: issue.end,
                        replacement: correction.clone(),
                    }],
                ));
            }
        }
        for issue in state
            .markdown_lint_issues
            .borrow()
            .iter()
            .filter(|issue| issue.start <= offset && offset <= issue.end)
        {
            if let Some(fix) = &issue.fix {
                fixes.push((
                    format!(
                        "Fix {}",
                        issue.rule_name.as_deref().unwrap_or("Markdown issue")
                    ),
                    fix.edits.clone(),
                ));
            }
            if let Some(rule) = &issue.rule_name {
                let action =
                    gio::SimpleAction::new(&format!("ignore-{}", group.list_actions().len()), None);
                action.connect_activate({
                    let state = Rc::downgrade(state);
                    let rule = rule.clone();
                    move |_, _| {
                        if let Some(state) = state.upgrade() {
                            let callbacks = state.markdown_lint_ignore_callbacks.borrow().clone();
                            for callback in callbacks {
                                callback(rule.clone());
                            }
                        }
                    }
                });
                menu.append(
                    Some(&format!("Ignore {rule} in this workspace")),
                    Some(&format!("diagnostics.{}", action.name())),
                );
                group.add_action(&action);
            }
        }
    }
    for (index, (label, mut edits)) in fixes.into_iter().enumerate() {
        let action = gio::SimpleAction::new(&format!("fix-{index}"), None);
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        action.connect_activate({
            let view = view.downgrade();
            let state = Rc::downgrade(state);
            let revision = state.upgrade().map(|state| state.revision.get());
            move |_, _| {
                let (Some(view), Some(state)) = (view.upgrade(), state.upgrade()) else {
                    return;
                };
                if !view.is_editable() || Some(state.revision.get()) != revision {
                    return;
                }
                let buffer = view
                    .buffer()
                    .downcast::<sourceview5::Buffer>()
                    .expect("source buffer");
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
                buffer.begin_user_action();
                for edit in &edits {
                    let mut start = iter_at_byte(&buffer, &text, edit.start);
                    let mut end = iter_at_byte(&buffer, &text, edit.end);
                    buffer.delete(&mut start, &mut end);
                    buffer.insert(&mut start, &edit.replacement);
                }
                buffer.end_user_action();
            }
        });
        menu.append(
            Some(&label),
            Some(&format!("diagnostics.{}", action.name())),
        );
        group.add_action(&action);
    }
    view.insert_action_group("diagnostics", Some(&group));
    view.set_extra_menu(Some(&menu));
}
