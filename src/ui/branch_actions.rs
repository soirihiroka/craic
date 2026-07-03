use super::dialogs::show_error_dialog;
use super::{AppState, refresh, request_provider_git_snapshot};
use crate::git::GitRepoHandle;
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
    let (workspace_key, git_handle) = match active_git_handle(state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            return;
        }
    };
    let action_git_handle = git_handle.clone();
    request_provider_git_snapshot(workspace_key.clone(), git_handle, {
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
                let git_handle = action_git_handle.clone();
                move |pop_after: bool| {
                    run_checkout_target(&state, git_handle.clone(), target.clone(), pop_after);
                }
            };

            if snapshot.changed_files.is_empty() {
                checkout_fn(false);
            } else {
                let branch = target.label();
                show_uncommitted_changes_dialog(
                    &state,
                    action_git_handle.clone(),
                    &branch,
                    &snapshot.branch,
                    checkout_fn,
                );
            }
        }
    });
}

fn checkout_target(
    git_handle: &GitRepoHandle,
    target: &BranchCheckoutTarget,
    callback: crate::git::OperationCallback<String>,
) {
    match target {
        BranchCheckoutTarget::Local(branch) => git_handle.checkout_branch(branch, callback),
        BranchCheckoutTarget::Remote {
            remote_branch,
            local_branch,
        } => git_handle.checkout_remote_branch(remote_branch, local_branch, callback),
        BranchCheckoutTarget::PullRequest(number) => {
            git_handle.checkout_pull_request(*number, callback)
        }
    }
}

fn run_checkout_target(
    state: &Rc<AppState>,
    git_handle: Arc<GitRepoHandle>,
    target: BranchCheckoutTarget,
    pop_after: bool,
) {
    let (sender, receiver) = mpsc::channel();
    let pop_handle = git_handle.clone();
    checkout_target(
        git_handle.as_ref(),
        &target,
        Box::new(move |result| {
            send_result_after_optional_pop(sender, pop_handle, result, pop_after);
        }),
    );
    poll_branch_action_result(
        state,
        receiver,
        "Checkout Failed",
        format!("Checked out {}.", target.label()),
    );
}

fn run_create_branch(
    state: &Rc<AppState>,
    git_handle: Arc<GitRepoHandle>,
    branch: String,
    pop_after: bool,
) {
    let (sender, receiver) = mpsc::channel();
    let pop_handle = git_handle.clone();
    git_handle.create_branch(
        &branch,
        Box::new(move |result| {
            send_result_after_optional_pop(sender, pop_handle, result, pop_after);
        }),
    );
    poll_branch_action_result(
        state,
        receiver,
        "Failed to Create Branch",
        format!("Created and checked out {branch}."),
    );
}

fn send_result_after_optional_pop(
    sender: mpsc::Sender<Result<String, String>>,
    git_handle: Arc<GitRepoHandle>,
    result: Result<String, String>,
    pop_after: bool,
) {
    if !pop_after || result.is_err() {
        let _ = sender.send(result);
        return;
    }

    git_handle.pop_stash(Box::new(move |pop_result| {
        let result = match pop_result {
            Ok(_) => result,
            Err(err) => Err(format!("Failed to apply stashed changes: {err}")),
        };
        let _ = sender.send(result);
    }));
}

fn poll_branch_action_result(
    state: &Rc<AppState>,
    receiver: mpsc::Receiver<Result<String, String>>,
    error_heading: &'static str,
    empty_success_message: String,
) {
    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let state = state.clone();
        move || match receiver.try_recv() {
            Ok(Ok(output)) => {
                super::broadcast_page_command(&state, super::pages::PageCommand::ClearSelection);
                if output.is_empty() {
                    refresh(&state, Some(empty_success_message.clone()));
                } else {
                    refresh(&state, Some(output));
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                show_error_dialog(&state.window, error_heading, &err);
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                show_error_dialog(
                    &state.window,
                    error_heading,
                    "Git operation did not return a result.",
                );
                gtk::glib::ControlFlow::Break
            }
        }
    });
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
    let (workspace_key, git_handle) = match active_git_handle(state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            return;
        }
    };
    let action_git_handle = git_handle.clone();
    request_provider_git_snapshot(workspace_key.clone(), git_handle, {
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
                let git_handle = action_git_handle.clone();
                move |pop_after: bool| {
                    run_create_branch(&state, git_handle.clone(), branch.clone(), pop_after);
                }
            };

            if snapshot.changed_files.is_empty() {
                create_fn(false);
            } else {
                show_uncommitted_changes_dialog(
                    &state,
                    action_git_handle.clone(),
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
    git_handle: Arc<GitRepoHandle>,
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
    let action = Rc::new(action);

    dialog.choose(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        let git_handle = git_handle.clone();
        let action = action.clone();
        move |response| {
            let pop_after = match response.as_str() {
                "bring" => true,
                "stash" => false,
                _ => return,
            };
            let (sender, receiver) = mpsc::channel();
            git_handle.stash_changes(Box::new(move |result| {
                let _ = sender.send(result.map(|_| pop_after));
            }));
            gtk::glib::timeout_add_local(Duration::from_millis(75), {
                let state = state.clone();
                let action = action.clone();
                move || match receiver.try_recv() {
                    Ok(Ok(pop_after)) => {
                        action(pop_after);
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(err)) => {
                        show_error_dialog(&state.window, "Stash Failed", &err);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(TryRecvError::Disconnected) => {
                        show_error_dialog(
                            &state.window,
                            "Stash Failed",
                            "Git stash did not return a result.",
                        );
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        }
    });
}

fn active_git_handle(state: &Rc<AppState>) -> Result<(String, Arc<GitRepoHandle>), String> {
    let workspace_key = state.workspace_ref.borrow().id.to_string();
    let system_id = state.system_ref.borrow().id.clone();
    let workspace_ref = state.workspace_ref.borrow().clone();
    let Some(git_handle) = super::git_handle_for_workspace(state, &system_id, &workspace_ref)
    else {
        return Err(format!(
            "Git is unavailable for workspace {}.",
            workspace_ref.display_name
        ));
    };
    Ok((workspace_key, git_handle))
}

fn active_workspace_matches(state: &Rc<AppState>, workspace_key: &str) -> bool {
    state.workspace_ref.borrow().id.to_string() == workspace_key
}
