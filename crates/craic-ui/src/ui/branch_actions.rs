use super::dialogs::show_error_dialog;
use super::{AppState, refresh, request_provider_git_snapshot};
use crate::git::{BranchInfo, GitRepoHandle, MergeResult, RepositorySnapshot};
use adw::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

#[derive(Clone, Debug)]
enum BranchCheckoutTarget {
    Local(String),
    Remote {
        remote_branch: String,
        local_branch: String,
    },
}

impl BranchCheckoutTarget {
    fn label(&self) -> String {
        match self {
            Self::Local(branch) => branch.clone(),
            Self::Remote { remote_branch, .. } => remote_branch.clone(),
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

    state.content.branch_picker.connect_footer_clicked({
        let state = state.clone();

        move || show_merge_branch_dialog(&state)
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

fn show_merge_branch_dialog(state: &Rc<AppState>) {
    let (workspace_key, git_handle) = match active_git_handle(state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            return;
        }
    };
    log::info!("merge branch dialog snapshot queued workspace={workspace_key}");
    request_provider_git_snapshot(workspace_key.clone(), git_handle.clone(), {
        let state = state.clone();
        move |response_key, result| {
            if response_key != workspace_key || !active_workspace_matches(&state, &workspace_key) {
                log::debug!(
                    "merge branch dialog snapshot ignored workspace={} response_workspace={}",
                    workspace_key,
                    response_key
                );
                return;
            }

            match result {
                Ok(snapshot) => present_merge_branch_dialog(
                    &state,
                    workspace_key.clone(),
                    git_handle.clone(),
                    snapshot,
                ),
                Err(err) => show_error_dialog(&state.window, "Repository Error", &err),
            }
        }
    });
}

fn present_merge_branch_dialog(
    state: &Rc<AppState>,
    workspace_key: String,
    git_handle: Arc<GitRepoHandle>,
    snapshot: RepositorySnapshot,
) {
    let current_branch = snapshot.branch;
    let branches = Rc::new(snapshot.branches);
    let selected = Rc::new(RefCell::new(
        branches
            .iter()
            .find(|branch| branch.is_default && branch.name != current_branch)
            .map(|branch| branch.name.clone()),
    ));
    let refreshing = Rc::new(Cell::new(false));

    let title = gtk::Label::new(Some(&format!("Merge into {current_branch}")));
    title.add_css_class("title");
    let header = adw::HeaderBar::builder()
        .title_widget(&title)
        .show_start_title_buttons(false)
        .show_end_title_buttons(true)
        .build();
    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Filter")
        .margin_top(12)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("navigation-sidebar");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&search_entry);
    content.append(&scroller);

    let status_icon = gtk::Image::builder()
        .icon_name("emblem-ok-symbolic")
        .pixel_size(16)
        .build();
    status_icon.add_css_class("success");
    let status_label = gtk::Label::builder()
        .halign(gtk::Align::Center)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let status = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    status.append(&status_icon);
    status.append(&status_label);

    let merge_spinner = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .visible(false)
        .build();
    let merge_label = gtk::Label::new(Some("Create a merge commit"));
    let merge_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    merge_content.append(&merge_spinner);
    merge_content.append(&merge_label);
    let merge_button = gtk::Button::builder()
        .child(&merge_content)
        .hexpand(true)
        .build();
    merge_button.add_css_class("suggested-action");
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    footer.append(&status);
    footer.append(&merge_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    toolbar.add_bottom_bar(&footer);
    let dialog = adw::Dialog::builder()
        .title(format!("Merge into {current_branch}"))
        .content_width(480)
        .content_height(520)
        .child(&toolbar)
        .default_widget(&merge_button)
        .focus_widget(&search_entry)
        .build();

    update_merge_selection(
        selected.borrow().as_deref(),
        &current_branch,
        &status_icon,
        &status_label,
        &merge_button,
    );
    refreshing.set(true);
    fill_merge_branch_list(
        &list,
        branches.as_ref(),
        &current_branch,
        "",
        selected.borrow().as_deref(),
    );
    refreshing.set(false);

    list.connect_row_selected({
        let current_branch = current_branch.clone();
        let selected = selected.clone();
        let refreshing = refreshing.clone();
        let status_icon = status_icon.clone();
        let status_label = status_label.clone();
        let merge_button = merge_button.clone();

        move |_, row| {
            if refreshing.get() {
                return;
            }
            let branch = row.map(|row| row.widget_name().to_string());
            selected.replace(branch.clone());
            update_merge_selection(
                branch.as_deref(),
                &current_branch,
                &status_icon,
                &status_label,
                &merge_button,
            );
        }
    });
    search_entry.connect_search_changed({
        let list = list.clone();
        let branches = branches.clone();
        let current_branch = current_branch.clone();
        let selected = selected.clone();
        let refreshing = refreshing.clone();

        move |entry| {
            refreshing.set(true);
            fill_merge_branch_list(
                &list,
                branches.as_ref(),
                &current_branch,
                entry.text().trim(),
                selected.borrow().as_deref(),
            );
            refreshing.set(false);
        }
    });
    merge_button.connect_clicked({
        let state = state.clone();
        let current_branch = current_branch.clone();
        let selected = selected.clone();
        let dialog = dialog.clone();
        let merge_button = merge_button.clone();
        let merge_spinner = merge_spinner.clone();
        let workspace_key = workspace_key.clone();
        let git_handle = git_handle.clone();

        move |_| {
            let Some(branch) = selected.borrow().clone() else {
                return;
            };
            if branch == current_branch {
                return;
            }
            merge_button.set_sensitive(false);
            merge_spinner.set_visible(true);
            run_merge_branch(
                &state,
                workspace_key.clone(),
                git_handle.clone(),
                &dialog,
                &merge_button,
                &merge_spinner,
                branch,
                current_branch.clone(),
            );
        }
    });

    log::info!(
        "merge branch dialog presented workspace={} current_branch={} branches={}",
        workspace_key,
        current_branch,
        branches.len()
    );
    dialog.present(Some(&state.window));
}

fn fill_merge_branch_list(
    list: &gtk::ListBox,
    branches: &[BranchInfo],
    current_branch: &str,
    filter: &str,
    selected: Option<&str>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let filter = filter.to_lowercase();
    let matches = |branch: &&BranchInfo| {
        filter.is_empty() || branch.name.to_lowercase().contains(filter.as_str())
    };
    let default = branches
        .iter()
        .filter(|branch| branch.is_default)
        .filter(matches)
        .collect::<Vec<_>>();
    let mut recent = branches
        .iter()
        .filter(|branch| !branch.is_default && branch.recent_order.is_some())
        .filter(matches)
        .collect::<Vec<_>>();
    recent.sort_by_key(|branch| branch.recent_order);
    recent.truncate(5);
    let recent_names = recent
        .iter()
        .map(|branch| branch.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let other = branches
        .iter()
        .filter(|branch| {
            !branch.is_default
                && !recent_names.contains(branch.name.as_str())
                && !branch.name.starts_with("github-desktop-")
        })
        .filter(matches)
        .collect::<Vec<_>>();

    let mut visible = 0;
    visible +=
        append_merge_branch_group(list, "Default branch", &default, current_branch, selected);
    visible +=
        append_merge_branch_group(list, "Recent branches", &recent, current_branch, selected);
    visible += append_merge_branch_group(list, "Other branches", &other, current_branch, selected);
    if visible == 0 {
        let label = gtk::Label::builder()
            .label("No matching branches.")
            .margin_top(18)
            .margin_bottom(18)
            .build();
        label.add_css_class("dim-label");
        let row = gtk::ListBoxRow::builder()
            .child(&label)
            .selectable(false)
            .build();
        row.set_activatable(false);
        list.append(&row);
    }
}

fn append_merge_branch_group(
    list: &gtk::ListBox,
    title: &str,
    branches: &[&BranchInfo],
    current_branch: &str,
    selected: Option<&str>,
) -> usize {
    if branches.is_empty() {
        return 0;
    }

    let header = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .margin_top(8)
        .margin_bottom(3)
        .margin_start(12)
        .margin_end(12)
        .build();
    header.add_css_class("heading");

    for (index, branch) in branches.iter().enumerate() {
        let icon = gtk::Image::builder()
            .icon_name(if branch.name == current_branch {
                "object-select-symbolic"
            } else {
                "branch-symbolic"
            })
            .pixel_size(16)
            .build();
        let label = gtk::Label::builder()
            .label(&branch.name)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(5)
            .margin_bottom(5)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&icon);
        content.append(&label);
        let row = gtk::ListBoxRow::builder().child(&content).build();
        row.set_widget_name(&branch.name);
        if index == 0 {
            row.set_header(Some(&header));
        }
        list.append(&row);
        if selected == Some(branch.name.as_str()) {
            list.select_row(Some(&row));
        }
    }
    branches.len()
}

fn update_merge_selection(
    selected: Option<&str>,
    current_branch: &str,
    status_icon: &gtk::Image,
    status_label: &gtk::Label,
    merge_button: &gtk::Button,
) {
    let valid = selected.is_some_and(|branch| branch != current_branch);
    status_icon.set_visible(valid);
    merge_button.set_sensitive(valid);
    match selected {
        Some(branch) if valid => {
            let branch = gtk::glib::markup_escape_text(branch);
            let current = gtk::glib::markup_escape_text(current_branch);
            status_label.set_markup(&format!(
                "This will merge <b>{branch}</b> into <b>{current}</b>."
            ));
        }
        Some(_) => status_label.set_label("Choose a different branch to merge."),
        None => status_label.set_label("Choose a branch to merge."),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_merge_branch(
    state: &Rc<AppState>,
    workspace_key: String,
    git_handle: Arc<GitRepoHandle>,
    dialog: &adw::Dialog,
    merge_button: &gtk::Button,
    merge_spinner: &adw::Spinner,
    branch: String,
    current_branch: String,
) {
    log::info!(
        "merge branch operation queued workspace={} source={} destination={}",
        workspace_key,
        branch,
        current_branch
    );
    let (sender, receiver) = mpsc::channel();
    git_handle.merge_branch(
        &branch,
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    gtk::glib::timeout_add_local(Duration::from_millis(75), {
        let state = state.clone();
        let dialog = dialog.clone();
        let merge_button = merge_button.clone();
        let merge_spinner = merge_spinner.clone();
        move || match receiver.try_recv() {
            Ok(result) => {
                if !active_workspace_matches(&state, &workspace_key) {
                    log::debug!(
                        "merge branch result ignored workspace={} source={} reason=workspace-changed",
                        workspace_key,
                        branch
                    );
                    let _ = dialog.close();
                    return gtk::glib::ControlFlow::Break;
                }

                match result {
                    Ok(MergeResult::Success) => {
                        log::info!(
                            "merge branch operation complete workspace={} source={} destination={} result=success",
                            workspace_key,
                            branch,
                            current_branch
                        );
                        let _ = dialog.close();
                        refresh(
                            &state,
                            Some(format!("Merged {branch} into {current_branch}.")),
                        );
                    }
                    Ok(MergeResult::AlreadyUpToDate) => {
                        log::info!(
                            "merge branch operation complete workspace={} source={} destination={} result=up-to-date",
                            workspace_key,
                            branch,
                            current_branch
                        );
                        let _ = dialog.close();
                        refresh(
                            &state,
                            Some(format!(
                                "{current_branch} is already up to date with {branch}."
                            )),
                        );
                    }
                    Ok(MergeResult::Conflicts(message)) => {
                        log::warn!(
                            "merge branch operation complete workspace={} source={} destination={} result=conflicts detail={}",
                            workspace_key,
                            branch,
                            current_branch,
                            message
                        );
                        let _ = dialog.close();
                        refresh(
                            &state,
                            Some(format!(
                                "Merge conflicts found. Resolve them to finish merging {branch}."
                            )),
                        );
                    }
                    Err(err) => {
                        log::warn!(
                            "merge branch operation failed workspace={} source={} destination={}: {}",
                            workspace_key,
                            branch,
                            current_branch,
                            err
                        );
                        merge_spinner.set_visible(false);
                        merge_button.set_sensitive(true);
                        refresh(&state, None);
                        show_error_dialog(&state.window, "Merge Failed", &err);
                    }
                }
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                merge_spinner.set_visible(false);
                merge_button.set_sensitive(true);
                show_error_dialog(
                    &state.window,
                    "Merge Failed",
                    "Git merge did not return a result.",
                );
                gtk::glib::ControlFlow::Break
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
