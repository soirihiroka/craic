use super::*;
use crate::ui::components::search::SearchOption;

pub(super) fn install(
    view: &sourceview5::View,
    buffer: &sourceview5::Buffer,
    root: &gtk::Box,
) -> (SearchPanel, sourceview5::SearchContext) {
    let panel = SearchPanel::new("Search");
    panel.set_key_capture_widget(view);
    panel.set_clear_on_close(false);
    panel.set_options_visible(true);
    panel.install_shortcuts(root);
    let settings = sourceview5::SearchSettings::new();
    settings.set_wrap_around(true);
    let context = sourceview5::SearchContext::new(buffer, Some(&settings));
    context.set_highlight(false);
    panel.connect_query_changed({
        let context = context.downgrade();
        let view = view.downgrade();
        move |query| {
            let (Some(context), Some(view)) = (context.upgrade(), view.upgrade()) else {
                return;
            };
            context
                .settings()
                .set_search_text((!query.is_empty()).then_some(query.as_str()));
            navigate(&context, &view, false, true);
        }
    });
    panel.connect_opened({
        let context = context.downgrade();
        move || {
            if let Some(context) = context.upgrade() {
                context.set_highlight(true);
            }
        }
    });
    panel.connect_closed({
        let context = context.downgrade();
        let view = view.downgrade();
        move || {
            if let Some(context) = context.upgrade() {
                context.set_highlight(false);
            }
            if let Some(view) = view.upgrade() {
                view.grab_focus();
            }
        }
    });
    for (option, property) in [
        (SearchOption::CaseSensitive, "case-sensitive"),
        (SearchOption::WholeWord, "at-word-boundaries"),
        (SearchOption::Regex, "regex-enabled"),
    ] {
        panel.connect_option_toggled(option, {
            let context = context.downgrade();
            let view = view.downgrade();
            move |enabled| {
                let (Some(context), Some(view)) = (context.upgrade(), view.upgrade()) else {
                    return;
                };
                context.settings().set_property(property, enabled);
                navigate(&context, &view, false, true);
            }
        });
    }
    panel.connect_next({
        let context = context.downgrade();
        let view = view.downgrade();
        move || {
            if let (Some(context), Some(view)) = (context.upgrade(), view.upgrade()) {
                navigate(&context, &view, false, false);
            }
        }
    });
    panel.connect_previous({
        let context = context.downgrade();
        let view = view.downgrade();
        move || {
            if let (Some(context), Some(view)) = (context.upgrade(), view.upgrade()) {
                navigate(&context, &view, true, false);
            }
        }
    });
    context.connect_occurrences_count_notify({
        let panel = panel.clone();
        move |context| update_status(context, &panel)
    });
    context.connect_regex_error_notify({
        let panel = panel.clone();
        move |context| update_status(context, &panel)
    });
    buffer.connect_mark_set({
        let context = context.downgrade();
        let panel = panel.clone();
        move |_, _, mark| {
            if matches!(mark.name().as_deref(), Some("insert" | "selection_bound")) {
                if let Some(context) = context.upgrade() {
                    update_status(&context, &panel);
                }
            }
        }
    });
    (panel, context)
}

fn navigate(
    context: &sourceview5::SearchContext,
    view: &sourceview5::View,
    backward: bool,
    from_start: bool,
) {
    let buffer = context.buffer();
    let iter = if from_start {
        buffer.start_iter()
    } else if let Some((start, end)) = buffer.selection_bounds() {
        if backward { start } else { end }
    } else {
        buffer.iter_at_offset(buffer.cursor_position())
    };
    let found = if backward {
        context.backward(&iter)
    } else {
        context.forward(&iter)
    };
    if let Some((start, mut end, _)) = found {
        buffer.select_range(&end, &start);
        view.scroll_to_iter(&mut end, 0.1, false, 0.0, 0.0);
    }
}

fn update_status(context: &sourceview5::SearchContext, panel: &SearchPanel) {
    if context.regex_error().is_some() {
        panel.set_status("Invalid regular expression");
        return;
    }
    let count = context.occurrences_count();
    let current = context
        .buffer()
        .selection_bounds()
        .map(|(start, end)| context.occurrence_position(&start, &end))
        .unwrap_or(0)
        .max(0);
    panel.set_status(&if count < 0 {
        "Searching…".to_string()
    } else {
        format!("{current}/{count}")
    });
}
