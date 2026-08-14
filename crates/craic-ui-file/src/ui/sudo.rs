use crate::system::capabilities::files::{
    FileAccess, FileSudoError, FileSudoErrorKind, FileSudoPassword,
};
use adw::prelude::*;
use craic_ui_core::ui::command_mailbox;
use gtk::gio;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) type SudoFileRetry = Rc<dyn Fn(Arc<dyn FileAccess>)>;
pub(crate) type SudoFileError = Rc<dyn Fn(String)>;

pub(crate) fn offer_retry(
    parent: gtk::Widget,
    file_access: Arc<dyn FileAccess>,
    heading: impl Into<String>,
    message: impl Into<String>,
    retry: SudoFileRetry,
    show_error: SudoFileError,
) {
    let heading = heading.into();
    let message = message.into();
    let cancel_message = message.clone();
    let dialog = adw::AlertDialog::builder()
        .heading(&heading)
        .body(format!("{message}\n\nTry this operation again with sudo?"))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("sudo", "Try with sudo");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let dialog_parent = parent.clone();
    dialog.choose(
        Some(&dialog_parent),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() == "sudo" {
                log::info!("file operation sudo retry approved heading={heading}");
                authorize(parent, file_access, None, retry, show_error.clone());
            } else {
                show_error(cancel_message.clone());
            }
        },
    );
}

fn authorize(
    parent: gtk::Widget,
    file_access: Arc<dyn FileAccess>,
    password: Option<FileSudoPassword>,
    retry: SudoFileRetry,
    show_error: SudoFileError,
) {
    let result_command = command_mailbox::once({
        let parent = parent.clone();
        let file_access = file_access.clone();
        let retry = retry.clone();
        let show_error = show_error.clone();
        move |result: Result<Arc<dyn FileAccess>, FileSudoError>| match result {
            Ok(access) => retry(access),
            Err(err)
                if matches!(
                    err.kind,
                    FileSudoErrorKind::PasswordRequired | FileSudoErrorKind::AuthenticationFailed
                ) =>
            {
                prompt_password(
                    parent.clone(),
                    file_access.clone(),
                    err.message,
                    retry.clone(),
                    show_error.clone(),
                );
            }
            Err(err) => show_error(err.message),
        }
    });
    std::thread::spawn(move || {
        result_command.send(file_access.sudo_access(password));
    });
}

fn prompt_password(
    parent: gtk::Widget,
    file_access: Arc<dyn FileAccess>,
    message: String,
    retry: SudoFileRetry,
    show_error: SudoFileError,
) {
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("Sudo Authentication")
        .body(if message.is_empty() {
            "Enter the sudo password for this workspace."
        } else {
            &message
        })
        .extra_child(&entry)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("authenticate", "Authenticate");
    dialog.set_default_response(Some("authenticate"));
    dialog.set_close_response("cancel");
    let dialog_parent = parent.clone();
    dialog.choose(
        Some(&dialog_parent),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() != "authenticate" {
                show_error("Sudo authentication canceled.".to_string());
                return;
            }
            let password = entry.text();
            let password = FileSudoPassword::new(password.as_bytes().to_vec());
            entry.set_text("");
            authorize(
                parent.clone(),
                file_access.clone(),
                Some(password),
                retry.clone(),
                show_error.clone(),
            );
        },
    );
}
