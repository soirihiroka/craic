use super::dialogs::show_error_dialog;
use super::{AppState, refresh, request_provider_git_snapshot};
use crate::system::capabilities::git::GitAccess;
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
enum BranchCheckoutTarget {
    Local(String),
    Remote {
        remote_branch: String,
        local_branch: String,
    },
    PullRequest(u32),
}

impl BranchCheckoutTarget {
    fn label(&self) -> String {
        match self {
            Self::Local(branch) => branch.clone(),
            Self::Remote { remote_branch, .. } => remote_branch.clone(),
            Self::PullRequest(number) => format!("pull request #{number}"),
        }
    }
}

pub(super) fn connect_branch_actions(state: &Rc<AppState>) {
    state.content.branch_picker.connect_item_activated({
        let state = state.clone();

        move |id| {
            if id.is_empty() {
                return;
            }

            if let Some(target) = parse_branch_picker_id(&id) {
                checkout_with_change_prompt(&state, target);
            }
        }
    });

    state.content.branch_picker.connect_action_clicked({
        let state = state.clone();

        move || show_new_branch_dialog(&state)
    });

    state.content.branch_picker.connect_opened({
        let state = state.clone();

        move || load_pull_requests(&state)
    });
}

fn parse_branch_picker_id(id: &str) -> Option<BranchCheckoutTarget> {
    if let Some(branch) = id.strip_prefix("branch:") {
        return Some(BranchCheckoutTarget::Local(branch.to_string()));
    }
    if let Some(remote_branch) = id.strip_prefix("remote:") {
        let local_branch = remote_branch
            .split_once('/')
            .map(|(_, branch)| branch)
            .unwrap_or(remote_branch)
            .to_string();
        return Some(BranchCheckoutTarget::Remote {
            remote_branch: remote_branch.to_string(),
            local_branch,
        });
    }
    if let Some(number) = id
        .strip_prefix("pr:")
        .and_then(|number| number.parse().ok())
    {
        return Some(BranchCheckoutTarget::PullRequest(number));
    }
    None
}

fn checkout_with_change_prompt(state: &Rc<AppState>, target: BranchCheckoutTarget) {
    let (workspace_key, git_access) = match active_git_access(state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            return;
        }
    };
    let action_git_access = git_access.clone();
    request_provider_git_snapshot(workspace_key.clone(), git_access, {
        let state = state.clone();
        let target = target.clone();
        move |response_key, result| {
            if response_key != workspace_key || !active_workspace_matches(&state, &workspace_key) {
                return;
            }

            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    show_error_dialog(&state.window, "Repository Error", &err);
                    return;
                }
            };

            let checkout_fn = {
                let state = state.clone();
                let target = target.clone();
                let git_access = action_git_access.clone();
                move |pop_after: bool| match checkout_target(git_access.as_ref(), &target) {
                    Ok(output) => {
                        if pop_after {
                            if let Err(err) = git_access.pop_stash() {
                                show_error_dialog(&state.window, "Failed to Apply Changes", &err);
                            }
                        }
                        super::broadcast_page_command(
                            &state,
                            super::pages::PageCommand::ClearSelection,
                        );
                        if output.is_empty() {
                            refresh(&state, Some(format!("Checked out {}.", target.label())));
                        } else {
                            refresh(&state, Some(output));
                        }
                    }
                    Err(err) => show_error_dialog(&state.window, "Checkout Failed", &err),
                }
            };

            if snapshot.changed_files.is_empty() {
                checkout_fn(false);
            } else {
                let branch = target.label();
                show_uncommitted_changes_dialog(
                    &state,
                    action_git_access.clone(),
                    &branch,
                    &snapshot.branch,
                    checkout_fn,
                );
            }
        }
    });
}

fn checkout_target(
    git_access: &dyn GitAccess,
    target: &BranchCheckoutTarget,
) -> Result<String, String> {
    match target {
        BranchCheckoutTarget::Local(branch) => git_access.checkout_branch(branch),
        BranchCheckoutTarget::Remote {
            remote_branch,
            local_branch,
        } => git_access.checkout_remote_branch(remote_branch, local_branch),
        BranchCheckoutTarget::PullRequest(number) => git_access.checkout_pull_request(*number),
    }
}

fn load_pull_requests(state: &Rc<AppState>) {
    let workspace_key = state.workspace_ref.borrow().id.to_string();
    let system_id = state.system_ref.borrow().id.clone();
    let workspace_ref = state.workspace_ref.borrow().clone();
    let Some(github_access) = state.providers.github(&system_id, &workspace_ref) else {
        state
            .content
            .set_pull_requests_error("GitHub pull requests are unavailable for this workspace.");
        return;
    };

    state.content.set_pull_requests_loading();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(github_access.open_pull_requests());
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let state = state.clone();
        move || {
            if !active_workspace_matches(&state, &workspace_key) {
                return gtk::glib::ControlFlow::Break;
            }

            match receiver.try_recv() {
                Ok(Ok(pull_requests)) => {
                    state.content.set_pull_requests(pull_requests);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    state.content.set_pull_requests_error(&err);
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    state
                        .content
                        .set_pull_requests_error("Pull request loader did not return a result.");
                    gtk::glib::ControlFlow::Break
                }
            }
        }
    });
}

fn show_new_branch_dialog(state: &Rc<AppState>) {
    let dialog = adw::AlertDialog::builder()
        .heading("New Branch")
        .body("Enter the name of the new branch:")
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name")
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        let entry = entry.clone();
        move |response| {
            if response != "create" {
                return;
            }

            let branch = entry.text().trim().to_string();
            if branch.is_empty() {
                return;
            }

            create_branch_with_change_prompt(&state, branch);
        }
    });
}

fn create_branch_with_change_prompt(state: &Rc<AppState>, branch: String) {
    let (workspace_key, git_access) = match active_git_access(state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            return;
        }
    };
    let action_git_access = git_access.clone();
    request_provider_git_snapshot(workspace_key.clone(), git_access, {
        let state = state.clone();
        let branch = branch.clone();
        move |response_key, result| {
            if response_key != workspace_key || !active_workspace_matches(&state, &workspace_key) {
                return;
            }

            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    show_error_dialog(&state.window, "Repository Error", &err);
                    return;
                }
            };

            let create_fn = {
                let state = state.clone();
                let branch = branch.clone();
                let git_access = action_git_access.clone();
                move |pop_after: bool| match git_access.create_branch(&branch) {
                    Ok(output) => {
                        if pop_after {
                            if let Err(err) = git_access.pop_stash() {
                                show_error_dialog(&state.window, "Failed to Apply Changes", &err);
                            }
                        }
                        super::broadcast_page_command(
                            &state,
                            super::pages::PageCommand::ClearSelection,
                        );
                        if output.is_empty() {
                            refresh(&state, Some(format!("Created and checked out {branch}.")));
                        } else {
                            refresh(&state, Some(output));
                        }
                    }
                    Err(err) => show_error_dialog(&state.window, "Failed to Create Branch", &err),
                }
            };

            if snapshot.changed_files.is_empty() {
                create_fn(false);
            } else {
                show_uncommitted_changes_dialog(
                    &state,
                    action_git_access.clone(),
                    &branch,
                    &snapshot.branch,
                    create_fn,
                );
            }
        }
    });
}

fn show_uncommitted_changes_dialog<F>(
    state: &Rc<AppState>,
    git_access: Arc<dyn GitAccess>,
    branch: &str,
    current_branch: &str,
    action: F,
) where
    F: Fn(bool) + 'static,
{
    let dialog = adw::AlertDialog::builder()
        .heading("Uncommitted Changes")
        .body("You have uncommitted changes. What would you like to do?")
        .build();
    dialog.add_response("bring", &format!("Bring changes to {branch}"));
    dialog.add_response("stash", &format!("Stash in {current_branch}"));
    dialog.add_response("cancel", "Cancel");
    dialog.set_default_response(Some("bring"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        let git_access = git_access.clone();
        move |response| match response.as_str() {
            "bring" => match git_access.stash_changes() {
                Ok(_) => action(true),
                Err(err) => show_error_dialog(&state.window, "Stash Failed", &err),
            },
            "stash" => match git_access.stash_changes() {
                Ok(_) => action(false),
                Err(err) => show_error_dialog(&state.window, "Stash Failed", &err),
            },
            _ => {}
        }
    });
}

fn active_git_access(state: &Rc<AppState>) -> Result<(String, Arc<dyn GitAccess>), String> {
    let workspace_key = state.workspace_ref.borrow().id.to_string();
    let system_id = state.system_ref.borrow().id.clone();
    let workspace_ref = state.workspace_ref.borrow().clone();
    let Some(git_access) = state.providers.git(&system_id, &workspace_ref) else {
        return Err(format!(
            "Git is unavailable for workspace {}.",
            workspace_ref.display_name
        ));
    };
    Ok((workspace_key, git_access))
}

fn active_workspace_matches(state: &Rc<AppState>, workspace_key: &str) -> bool {
    state.workspace_ref.borrow().id.to_string() == workspace_key
}
