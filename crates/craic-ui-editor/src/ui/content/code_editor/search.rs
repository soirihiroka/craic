use super::*;
use crate::ui::components::search::SearchOption;

pub(super) fn install(view: &Viewport, root: &gtk::Box) {
    let panel = &view.state().search_panel;
    panel.set_key_capture_widget(view);
    panel.set_clear_on_close(false);
    panel.set_options_visible(true);
    panel.install_shortcuts(root);
    panel.connect_query_changed({
        let view = view.downgrade();
        move |_| {
            if let Some(view) = view.upgrade() {
                update(&view, true);
                view.refresh();
            }
        }
    });
    panel.connect_opened({
        let view = view.downgrade();
        move || {
            if let Some(view) = view.upgrade() {
                view.state().search_open.set(true);
                update(&view, true);
                view.refresh();
            }
        }
    });
    panel.connect_closed({
        let view = view.downgrade();
        move || {
            if let Some(view) = view.upgrade() {
                view.state().search_open.set(false);
                view.state().editor.borrow_mut().clear_search();
                view.grab_focus();
                view.queue_draw();
            }
        }
    });
    for option in [
        SearchOption::CaseSensitive,
        SearchOption::WholeWord,
        SearchOption::Regex,
    ] {
        panel.connect_option_toggled(option, {
            let view = view.downgrade();
            move |enabled| {
                let Some(view) = view.upgrade() else { return };
                let mut options = view.state().search_options.get();
                match option {
                    SearchOption::CaseSensitive => options.case_sensitive = enabled,
                    SearchOption::WholeWord => options.whole_word = enabled,
                    SearchOption::Regex => options.regex = enabled,
                }
                view.state().search_options.set(options);
                update(&view, true);
                view.refresh();
            }
        });
    }
    for forward in [true, false] {
        let view = view.downgrade();
        let navigate = move || {
            if let Some(view) = view.upgrade() {
                view.state().editor.borrow_mut().step_search(forward);
                update(&view, false);
                view.refresh();
            }
        };
        if forward {
            panel.connect_next(navigate);
        } else {
            panel.connect_previous(navigate);
        }
    }
}

pub(super) fn update(view: &Viewport, rebuild: bool) {
    let state = view.state();
    let panel = &state.search_panel;
    if !state.search_open.get() {
        return;
    }
    let mut editor = state.editor.borrow_mut();
    if rebuild
        && editor
            .set_search(&panel.query(), state.search_options.get())
            .is_err()
    {
        panel.set_status("Invalid regular expression");
        return;
    }
    let current = editor.active_search_match.map_or(0, |index| index + 1);
    let total = editor.search_matches.len();
    drop(editor);
    panel.set_status(&format!("{current}/{total}"));
}
