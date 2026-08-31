use adw::prelude::*;
use craic_ui_markdown::LinkTarget;
use gtk::gio;

use super::super::super::{PageCommand, PageContext};
use crate::system::capabilities::terminal_link::TerminalLinkTarget;

pub(super) fn activate(ctx: &PageContext, workspace_root: &str, target: LinkTarget) {
    match target {
        LinkTarget::Url(url) => confirm_open_url(ctx.clone(), url),
        LinkTarget::File { path, line, column } => {
            open_file(ctx, workspace_root, &path, line, column)
        }
    }
}

fn confirm_open_url(ctx: PageContext, url: String) {
    let Some(url_opener) = ctx.url_opener() else {
        let message = ctx.url_opener_unavailable_message();
        log::warn!("Codex chat link activation failed reason=no-url-opener url={url}");
        ctx.show_error("Open Link Failed", &message);
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Open Link?")
        .body(&url)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Open");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = ctx.window();
    dialog.choose(
        parent.as_ref(),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() != "open" {
                log::debug!("Codex chat link activation cancelled url={url}");
                return;
            }

            match url_opener
                .resolve_url(&url)
                .and_then(|effect| ctx.execute_effect(effect))
            {
                Ok(message) => {
                    log::info!("Codex chat link opened url={url} message={message}");
                    ctx.show_toast(&message);
                }
                Err(err) => {
                    log::warn!("Codex chat link activation failed url={url}: {err}");
                    ctx.show_error("Open Link Failed", &err);
                }
            }
        },
    );
}

fn open_file(
    ctx: &PageContext,
    workspace_root: &str,
    path: &str,
    line: Option<usize>,
    column: Option<usize>,
) {
    let Some(terminal_links) = ctx.terminal_links() else {
        let message = "File-link navigation is unavailable for this workspace.";
        log::warn!(
            "Codex chat file activation failed path={path} launch_dir={workspace_root} reason=no-terminal-link-capability"
        );
        ctx.show_error("Open File Failed", message);
        return;
    };

    let path = path.strip_prefix("file://").unwrap_or(path);
    let target = match terminal_links.resolve_file(workspace_root, path) {
        Ok(target) => target,
        Err(err) => {
            log::warn!(
                "Codex chat file activation failed path={path} launch_dir={workspace_root}: {err}"
            );
            ctx.show_error("Open File Failed", &err);
            return;
        }
    };

    match target {
        TerminalLinkTarget::Workspace(path) => {
            log::info!(
                "Codex chat file activation dispatched path={} resolved_path={} line={line:?} column={column:?}",
                path.display(),
                path.relative_or_empty()
            );
            ctx.dispatch_command(PageCommand::OpenFileLocation {
                path: path.relative_or_empty().to_string(),
                line,
                column,
            });
        }
        TerminalLinkTarget::External(path) => {
            log::info!(
                "Codex chat external file activation requesting new window path={} line={line:?} column={column:?}",
                path.absolute
            );
            ctx.open_external_terminal_path(&path, line, column);
        }
    }
}
