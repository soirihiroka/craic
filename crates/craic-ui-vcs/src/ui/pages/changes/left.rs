use crate::ui::sidebar::changes_panel::ChangesPanel;
use crate::ui::sidebar::commit_panel::CommitPanel;
use adw::prelude::*;

pub fn build() -> (CommitPanel, ChangesPanel) {
    let commit_form = CommitPanel::new();
    let changes_panel = ChangesPanel::new(&commit_form);
    (commit_form, changes_panel)
}

pub fn text_view_text(text_view: &gtk::TextView) -> String {
    let buffer = text_view.buffer();
    let (start, end) = buffer.bounds();
    buffer.text(&start, &end, false).to_string()
}
