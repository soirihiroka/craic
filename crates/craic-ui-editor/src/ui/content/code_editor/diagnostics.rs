use super::*;
use crate::markdown_lint::MarkdownLintEdit;
use gtk::{gdk, gio};

pub(super) fn tooltip(state: &EditorState, offset: usize, tooltip: &gtk::Tooltip) -> bool {
    let mut messages = Vec::new();
    for issue in state
        .spellcheck_issues
        .borrow()
        .iter()
        .filter(|issue| (issue.start..issue.end).contains(&offset))
    {
        messages.push(format!("Spelling: {}", issue.word));
    }
    for issue in state
        .markdown_lint_issues
        .borrow()
        .iter()
        .filter(|issue| (issue.start..issue.end).contains(&offset))
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

pub(super) fn show_menu(view: &Viewport, offset: usize, x: i32, y: i32) {
    let state = view.state();
    let (editable, selected) = {
        let editor = state.editor.borrow();
        (editor.editable, !editor.selection.is_empty())
    };
    let menu = gio::Menu::new();
    let clipboard = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();
    for (action, label, key, enabled) in [
        ("copy", "Copy", gdk::Key::c, selected),
        ("cut", "Cut", gdk::Key::x, editable && selected),
        ("paste", "Paste", gdk::Key::v, editable),
    ] {
        let item = gio::SimpleAction::new(action, None);
        item.set_enabled(enabled);
        item.connect_activate({
            let view = view.downgrade();
            move |_, _| {
                if let Some(view) = view.upgrade() {
                    view.clipboard_action(key);
                }
            }
        });
        actions.add_action(&item);
        clipboard.append(Some(label), Some(&format!("editor.{action}")));
    }
    menu.append_section(None, &clipboard);
    if editable {
        let fixes = gio::Menu::new();
        for issue in state
            .spellcheck_issues
            .borrow()
            .iter()
            .filter(|issue| issue.start <= offset && offset <= issue.end)
        {
            for correction in &issue.corrections {
                add_fix(
                    view,
                    &actions,
                    &fixes,
                    &format!("Change to “{correction}”"),
                    vec![MarkdownLintEdit {
                        start: issue.start,
                        end: issue.end,
                        replacement: correction.clone(),
                    }],
                );
            }
        }
        for issue in state
            .markdown_lint_issues
            .borrow()
            .iter()
            .filter(|issue| issue.start <= offset && offset <= issue.end)
        {
            if let Some(fix) = &issue.fix {
                let name = issue.rule_name.as_deref().unwrap_or("Markdown issue");
                add_fix(
                    view,
                    &actions,
                    &fixes,
                    &format!("Fix {name}"),
                    fix.edits.clone(),
                );
            }
            if let Some(rule) = &issue.rule_name {
                let action = gio::SimpleAction::new(
                    &format!("ignore-{}", actions.list_actions().len()),
                    None,
                );
                action.connect_activate({
                    let view = view.downgrade();
                    let rule = rule.clone();
                    move |_, _| {
                        if let Some(view) = view.upgrade() {
                            let callbacks =
                                view.state().markdown_lint_ignore_callbacks.borrow().clone();
                            for callback in callbacks {
                                callback(rule.clone());
                            }
                        }
                    }
                });
                fixes.append(
                    Some(&format!("Ignore {rule} in this workspace")),
                    Some(&format!("editor.{}", action.name())),
                );
                actions.add_action(&action);
            }
        }
        if fixes.n_items() > 0 {
            menu.append_section(None, &fixes);
        }
    }
    if let Some(previous) = view.first_child().and_downcast::<gtk::PopoverMenu>() {
        previous.popdown();
        if previous.parent().is_some() {
            previous.unparent();
        }
    }
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.insert_action_group("editor", Some(&actions));
    popover.set_parent(view);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
    popover.popup();
}

fn add_fix(
    view: &Viewport,
    actions: &gio::SimpleActionGroup,
    menu: &gio::Menu,
    label: &str,
    mut edits: Vec<MarkdownLintEdit>,
) {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    let action = gio::SimpleAction::new(&format!("fix-{}", actions.list_actions().len()), None);
    let revision = view.state().revision.get();
    let weak = view.downgrade();
    action.connect_activate(move |_, _| {
        let Some(view) = weak.upgrade() else { return };
        if view.state().revision.get() != revision {
            return;
        }
        let mut editor = view.state().editor.borrow_mut();
        let (Some(last), Some(first)) = (edits.first(), edits.last()) else {
            return;
        };
        let range = first.start..last.end;
        let Some(original) = editor.document.text().get(range.clone()) else {
            return;
        };
        let mut replacement = original.to_owned();
        let mut previous = last.end;
        for edit in &edits {
            if edit.start < range.start
                || edit.end < edit.start
                || edit.end > previous
                || replacement
                    .get(edit.start - range.start..edit.end - range.start)
                    .is_none()
            {
                return;
            }
            replacement.replace_range(
                edit.start - range.start..edit.end - range.start,
                &edit.replacement,
            );
            previous = edit.start;
        }
        let changed = editor.replace_text(Some(range), &replacement);
        drop(editor);
        if changed {
            view.document_changed(true);
        }
    });
    menu.append(Some(label), Some(&format!("editor.{}", action.name())));
    actions.add_action(&action);
}
