use adw::prelude::*;

pub(super) fn show_error_dialog(window: &adw::ApplicationWindow, heading: &str, message: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(message));
    dialog.add_response("ok", "OK");
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(Some(window));
}

pub(super) fn show_startup_crash_dialog(
    window: &adw::ApplicationWindow,
    notice: &crate::crash_log::CrashNotice,
) {
    let message = format!(
        "{}\n\nDump file:\n{}",
        notice.summary,
        notice.path.display()
    );
    let dialog = adw::AlertDialog::new(Some("Craic Crashed Last Time"), Some(&message));
    dialog.add_response("ok", "OK");
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(Some(window));
}
