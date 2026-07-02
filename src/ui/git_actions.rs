use super::dialogs::show_error_dialog;
use super::{
    AppState, refresh, refresh_active_repo_metadata, refresh_without_toast,
    request_provider_git_snapshot,
};
use crate::git::RepositorySnapshot;
use crate::github::{GitHubAuthAccount, GitHubPublishRepositoryRequest, GitHubRepositoryOwner};
use crate::system::capabilities::{git::GitAccess, github::GitHubAccess};
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub(super) enum GitAction {
    Fetch(Option<String>),
    Push,
    Pull,
    PullPush,
    Publish(String, String),
}

enum GitActionEvent {
    Progress(String),
    Finished(Result<String, String>),
}

pub(super) fn execute_git_action(state: &Rc<AppState>, action: GitAction) {
    if state.git_action_running.get() {
        return;
    }

    let state = state.clone();
    let (workspace_key, git_access) = match active_git_access(&state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            refresh(&state, None);
            return;
        }
    };
    let action = action.clone();
    let action_git_access = git_access.clone();
    request_provider_git_snapshot(
        workspace_key.clone(),
        git_access,
        move |response_key, result| match result {
            Ok(snapshot) => {
                if response_key != workspace_key
                    || !active_workspace_matches(&state, &workspace_key)
                {
                    return;
                }
                execute_git_action_with_snapshot(
                    &state,
                    snapshot,
                    action.clone(),
                    action_git_access.clone(),
                );
            }
            Err(err) => {
                if response_key == workspace_key && active_workspace_matches(&state, &workspace_key)
                {
                    show_error_dialog(&state.window, "Repository Error", &err);
                    refresh(&state, None);
                }
            }
        },
    );
}

pub(super) fn run_git_action(state: &Rc<AppState>) {
    let state = state.clone();
    let (workspace_key, git_access) = match active_git_access(&state) {
        Ok(access) => access,
        Err(err) => {
            show_error_dialog(&state.window, "Repository Error", &err);
            refresh(&state, None);
            return;
        }
    };
    let action_git_access = git_access.clone();
    request_provider_git_snapshot(
        workspace_key.clone(),
        git_access,
        move |response_key, result| {
            if response_key != workspace_key || !active_workspace_matches(&state, &workspace_key) {
                return;
            }

            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    show_error_dialog(&state.window, "Repository Error", &err);
                    refresh(&state, None);
                    return;
                }
            };

            let remote = snapshot.remote_name.clone();
            let action = if !snapshot.has_upstream {
                if let Some(remote_name) = remote.clone() {
                    GitAction::Publish(remote_name, snapshot.branch.clone())
                } else {
                    show_publish_repository_dialog(&state, snapshot);
                    return;
                }
            } else if snapshot.behind > 0 {
                if snapshot.ahead > 0 {
                    GitAction::PullPush
                } else {
                    GitAction::Pull
                }
            } else if snapshot.ahead > 0 {
                GitAction::Push
            } else {
                GitAction::Fetch(remote.clone())
            };

            execute_git_action_with_snapshot(&state, snapshot, action, action_git_access.clone());
        },
    );
}

fn show_publish_repository_dialog(state: &Rc<AppState>, snapshot: RepositorySnapshot) {
    let workspace = state.workspace_ref.borrow().clone();
    let system_id = state.system_ref.borrow().id.clone();
    let Some(github_access) = state.providers.github(&system_id, &workspace) else {
        show_error_dialog(
            &state.window,
            "Publish Repository Failed",
            "GitHub CLI access is unavailable for this workspace.",
        );
        return;
    };

    state.git_action_running.set(true);
    state.content.clear_git_action_progress();
    state.content.update(&snapshot, None, true, false);

    let dialog = adw::PreferencesDialog::builder()
        .title("Publish Repository")
        .content_width(620)
        .content_height(520)
        .build();

    let status_spinner = adw::Spinner::new();
    status_spinner.set_size_request(18, 18);
    status_spinner.set_valign(gtk::Align::Center);
    let status_row = adw::ActionRow::builder()
        .title("Loading GitHub")
        .subtitle("Loading accounts and owners")
        .build();
    status_row.add_suffix(&status_spinner);
    let status_group = adw::PreferencesGroup::new();
    status_group.add(&status_row);

    let loading_model = gtk::StringList::new(&["Loading..."]);
    let account_row = adw::ComboRow::builder()
        .title("Authenticated Account")
        .model(&loading_model)
        .selected(0)
        .sensitive(false)
        .build();

    let owner_choices = Rc::new(std::cell::RefCell::new(Vec::<GitHubRepositoryOwner>::new()));
    let owners_by_account = Rc::new(std::cell::RefCell::new(
        Vec::<Vec<GitHubRepositoryOwner>>::new(),
    ));
    let owner_model = gtk::StringList::new(&["Loading..."]);
    let owner_row = adw::ComboRow::builder()
        .title("Repository Owner")
        .model(&owner_model)
        .selected(0)
        .sensitive(false)
        .build();

    account_row.connect_selected_notify({
        let owner_row = owner_row.clone();
        let owner_choices = owner_choices.clone();
        let owners_by_account = owners_by_account.clone();

        move |row| {
            let index = row.selected() as usize;
            let owners = owners_by_account
                .borrow()
                .get(index)
                .cloned()
                .unwrap_or_default();
            let model = owner_string_list(&owners);
            owner_choices.replace(owners);
            owner_row.set_model(Some(&model));
            owner_row.set_selected(0);
        }
    });

    let name_row = adw::EntryRow::builder().title("Repository Name").build();
    name_row.set_text(&default_publish_repository_name(state, &snapshot));
    name_row.set_sensitive(false);
    let name_check_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    name_check_icon.add_css_class("warning");
    name_check_icon.set_pixel_size(16);
    let name_check_label = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .visible(false)
        .build();
    name_check_label.add_css_class("warning");
    let name_check_message = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    name_check_message.append(&name_check_icon);
    name_check_message.append(&name_check_label);
    name_row.add_suffix(&name_check_message);
    name_row.connect_changed({
        let name_check_message = name_check_message.clone();
        let name_check_label = name_check_label.clone();

        move |_| {
            set_publish_name_check_message(&name_check_message, &name_check_label, None);
        }
    });
    owner_row.connect_selected_notify({
        let name_check_message = name_check_message.clone();
        let name_check_label = name_check_label.clone();

        move |_| {
            set_publish_name_check_message(&name_check_message, &name_check_label, None);
        }
    });

    let private_row = adw::ActionRow::builder()
        .title("Private Repository")
        .build();
    let private_switch = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    private_row.add_suffix(&private_switch);
    private_row.set_sensitive(false);

    let destination_group = adw::PreferencesGroup::new();
    destination_group.set_title("Destination");
    destination_group.add(&account_row);
    destination_group.add(&owner_row);

    let repository_group = adw::PreferencesGroup::new();
    repository_group.set_title("Repository");
    repository_group.add(&name_row);
    repository_group.add(&private_row);

    let cancel_button = gtk::Button::builder().label("Cancel").build();
    let publish_spinner = adw::Spinner::new();
    publish_spinner.set_size_request(16, 16);
    publish_spinner.set_visible(false);
    let publish_label = gtk::Label::new(Some("Publish Repository"));
    let publish_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    publish_content.append(&publish_spinner);
    publish_content.append(&publish_label);
    let publish_button = gtk::Button::builder()
        .child(&publish_content)
        .sensitive(false)
        .build();
    publish_button.add_css_class("suggested-action");

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    actions.append(&cancel_button);
    actions.append(&publish_button);
    let action_group = adw::PreferencesGroup::new();
    action_group.add(&actions);

    let page = adw::PreferencesPage::new();
    page.set_title("Publish Repository");
    page.set_icon_name(Some("github-symbolic"));
    page.add(&status_group);
    page.add(&destination_group);
    page.add(&repository_group);
    page.add(&action_group);
    dialog.add(&page);

    let state = state.clone();
    cancel_button.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
        }
    });

    publish_button.connect_clicked({
        let github_access = github_access.clone();
        let state = state.clone();
        let dialog = dialog.clone();
        let snapshot = snapshot.clone();
        let owner_choices = owner_choices.clone();
        let owner_row = owner_row.clone();
        let name_row = name_row.clone();
        let name_check_message = name_check_message.clone();
        let name_check_label = name_check_label.clone();
        let private_switch = private_switch.clone();
        let status_row = status_row.clone();
        let publish_button = publish_button.clone();
        let publish_spinner = publish_spinner.clone();
        let publish_label = publish_label.clone();

        move |_| {
            let name = name_row.text().trim().to_string();
            if name.is_empty() {
                set_publish_name_check_message(
                    &name_check_message,
                    &name_check_label,
                    Some("Repository name required."),
                );
                return;
            }
            set_publish_name_check_message(&name_check_message, &name_check_label, None);

            let Some(owner) = owner_choices
                .borrow()
                .get(owner_row.selected() as usize)
                .cloned()
            else {
                status_row.set_title("Repository owner required");
                status_row.set_subtitle("");
                return;
            };

            let request = GitHubPublishRepositoryRequest {
                host: owner.host,
                auth_login: owner.auth_login,
                owner: owner.owner,
                name,
                private: private_switch.is_active(),
            };

            set_publish_button_loading(
                &publish_button,
                &publish_spinner,
                &publish_label,
                true,
                "Checking availability...",
            );
            let (sender, receiver) = mpsc::channel();
            let github_access_for_check = github_access.clone();
            let request_for_check = request.clone();
            thread::spawn(move || {
                let result = github_access_for_check
                    .repository_exists(&request_for_check)
                    .map(|exists| (exists, request_for_check));
                let _ = sender.send(result);
            });

            gtk::glib::timeout_add_local(Duration::from_millis(100), {
                let state = state.clone();
                let snapshot = snapshot.clone();
                let github_access = github_access.clone();
                let dialog = dialog.clone();
                let publish_button = publish_button.clone();
                let publish_spinner = publish_spinner.clone();
                let publish_label = publish_label.clone();
                let name_check_message = name_check_message.clone();
                let name_check_label = name_check_label.clone();

                move || match receiver.try_recv() {
                    Ok(Ok((true, request))) => {
                        set_publish_name_check_message(
                            &name_check_message,
                            &name_check_label,
                            Some(&format!(
                                "Repository {}/{} already exists.",
                                request.owner, request.name
                            )),
                        );
                        set_publish_button_loading(
                            &publish_button,
                            &publish_spinner,
                            &publish_label,
                            false,
                            "Publish Repository",
                        );
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Ok((false, request))) => {
                        dialog.close();
                        execute_publish_repository_action(
                            &state,
                            snapshot.clone(),
                            github_access.clone(),
                            request,
                        );
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(err)) => {
                        set_publish_name_check_message(
                            &name_check_message,
                            &name_check_label,
                            Some(&err),
                        );
                        set_publish_button_loading(
                            &publish_button,
                            &publish_spinner,
                            &publish_label,
                            false,
                            "Publish Repository",
                        );
                        gtk::glib::ControlFlow::Break
                    }
                    Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(TryRecvError::Disconnected) => {
                        set_publish_name_check_message(
                            &name_check_message,
                            &name_check_label,
                            Some("Check stopped unexpectedly."),
                        );
                        set_publish_button_loading(
                            &publish_button,
                            &publish_spinner,
                            &publish_label,
                            false,
                            "Publish Repository",
                        );
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        }
    });

    dialog.connect_closed({
        let state = state.clone();
        let snapshot = snapshot.clone();

        move |_| {
            if state.git_action_running.get() {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
            }
        }
    });
    dialog.present(Some(&state.window));

    let (sender, receiver) = mpsc::channel();
    let preferred_auth_account = github_access.preferred_auth_account();
    let github_access_for_load = github_access.clone();
    thread::spawn(move || {
        let result = load_publish_repository_options(github_access_for_load);
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let state = state.clone();
        let snapshot = snapshot.clone();
        let status_spinner = status_spinner.clone();
        let status_row = status_row.clone();
        let account_row = account_row.clone();
        let owner_row = owner_row.clone();
        let name_row = name_row.clone();
        let private_row = private_row.clone();
        let publish_button = publish_button.clone();
        let owners_by_account = owners_by_account.clone();
        let owner_choices = owner_choices.clone();
        let preferred_auth_account = preferred_auth_account.clone();

        move || match receiver.try_recv() {
            Ok(Ok((accounts, owners))) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                status_spinner.set_visible(false);
                status_row.set_title("Ready");
                status_row.set_subtitle("Choose a destination.");

                let account_labels = accounts.iter().map(auth_account_label).collect::<Vec<_>>();
                let account_label_refs = account_labels
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let account_model = gtk::StringList::new(&account_label_refs);
                account_row.set_model(Some(&account_model));
                let selected_account_index = preferred_auth_account
                    .as_ref()
                    .and_then(|preferred| accounts.iter().position(|account| account == preferred))
                    .unwrap_or_default();
                account_row.set_selected(selected_account_index as u32);
                account_row.set_sensitive(true);

                owners_by_account.replace(owners.clone());
                let initial_owners = owners
                    .get(selected_account_index)
                    .cloned()
                    .unwrap_or_default();
                let owner_model = owner_string_list(&initial_owners);
                owner_choices.replace(initial_owners);
                owner_row.set_model(Some(&owner_model));
                owner_row.set_selected(0);
                owner_row.set_sensitive(true);
                name_row.set_sensitive(true);
                private_row.set_sensitive(true);
                publish_button.set_sensitive(true);
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                status_spinner.set_visible(false);
                status_row.set_title("GitHub loading failed");
                status_row.set_subtitle(&err);
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                status_spinner.set_visible(false);
                status_row.set_title("GitHub loading failed");
                status_row.set_subtitle("Loading stopped unexpectedly.");
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn set_publish_name_check_message(message: &gtk::Box, label: &gtk::Label, text: Option<&str>) {
    let Some(text) = text else {
        message.set_visible(false);
        label.set_label("");
        label.set_tooltip_text(None);
        return;
    };

    message.set_visible(true);
    label.set_visible(true);
    label.set_label(text);
    label.set_tooltip_text(Some(text));
}

fn load_publish_repository_options(
    github_access: Arc<dyn GitHubAccess>,
) -> Result<(Vec<GitHubAuthAccount>, Vec<Vec<GitHubRepositoryOwner>>), String> {
    let accounts = github_access.authenticated_accounts()?;
    if accounts.is_empty() {
        return Err(
            "No authenticated GitHub accounts were found. Run gh auth login first.".to_string(),
        );
    }

    let mut owners_by_account = Vec::new();
    for account in &accounts {
        match github_access.repository_owners(account) {
            Ok(owners) if !owners.is_empty() => owners_by_account.push(owners),
            Ok(_) | Err(_) => owners_by_account.push(vec![GitHubRepositoryOwner {
                host: account.host.clone(),
                auth_login: account.login.clone(),
                owner: account.login.clone(),
            }]),
        }
    }

    Ok((accounts, owners_by_account))
}

fn set_publish_button_loading(
    button: &gtk::Button,
    spinner: &adw::Spinner,
    label: &gtk::Label,
    loading: bool,
    text: &str,
) {
    spinner.set_visible(loading);
    label.set_label(text);
    button.set_sensitive(!loading);
}

fn execute_publish_repository_action(
    state: &Rc<AppState>,
    snapshot: RepositorySnapshot,
    github_access: Arc<dyn GitHubAccess>,
    request: GitHubPublishRepositoryRequest,
) {
    if state.git_action_running.get() {
        return;
    }

    let (sender, receiver) = mpsc::channel();
    state.git_action_running.set(true);
    state.content.clear_git_action_progress();
    state.content.update(&snapshot, None, true, false);
    log::info!(
        "starting github repository publish owner={} name={}",
        request.owner,
        request.name
    );

    thread::spawn(move || {
        let result = github_access.publish_repository(&request);
        let _ = sender.send(GitActionEvent::Finished(result));
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let state = state.clone();
        move || match receiver.try_recv() {
            Ok(GitActionEvent::Finished(Ok(message))) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                log::info!("github repository publish completed: {message}");
                refresh_active_repo_metadata(&state, None);
                refresh_without_toast(&state, Some(message));
                gtk::glib::ControlFlow::Break
            }
            Ok(GitActionEvent::Finished(Err(err))) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                log::warn!("github repository publish failed: {err}");
                show_error_dialog(&state.window, "Publish Repository Failed", &err);
                refresh(&state, None);
                gtk::glib::ControlFlow::Break
            }
            Ok(GitActionEvent::Progress(progress)) => {
                state.content.set_git_action_progress(&progress);
                gtk::glib::ControlFlow::Continue
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                show_error_dialog(
                    &state.window,
                    "Publish Repository Failed",
                    "Publish worker stopped unexpectedly.",
                );
                refresh(&state, None);
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn auth_account_label(account: &GitHubAuthAccount) -> String {
    if account.host.eq_ignore_ascii_case("github.com") {
        account.login.clone()
    } else {
        format!("{} on {}", account.login, account.host)
    }
}

fn owner_string_list(owners: &[GitHubRepositoryOwner]) -> gtk::StringList {
    let labels = owners
        .iter()
        .map(|owner| {
            if owner.owner == owner.auth_login {
                format!("{} (user)", owner.owner)
            } else {
                format!("{} (organization)", owner.owner)
            }
        })
        .collect::<Vec<_>>();
    let label_refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    gtk::StringList::new(&label_refs)
}

fn default_publish_repository_name(state: &Rc<AppState>, snapshot: &RepositorySnapshot) -> String {
    if !snapshot.name.trim().is_empty() {
        return snapshot.name.clone();
    }
    state
        .repo_path
        .borrow()
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "repository".to_string())
}

fn execute_git_action_with_snapshot(
    state: &Rc<AppState>,
    snapshot: RepositorySnapshot,
    action: GitAction,
    git_access: Arc<dyn GitAccess>,
) {
    let refresh_repo_metadata_on_success = matches!(action, GitAction::Pull | GitAction::PullPush);
    let show_success_toast = !matches!(
        &action,
        GitAction::Push | GitAction::PullPush | GitAction::Publish(_, _)
    );
    let (sender, receiver) = mpsc::channel();

    state.git_action_running.set(true);
    state.content.clear_git_action_progress();
    state.content.update(&snapshot, None, true, false);
    log::debug!(
        "starting git action {:?} in workspace {}",
        action,
        state.workspace_ref.borrow().display_name
    );

    thread::spawn({
        let action = action.clone();
        let git_access = git_access.clone();
        move || {
            let message = match action {
                GitAction::Push => match git_access.push() {
                    Ok(output) if output.is_empty() => Ok("Push complete.".to_string()),
                    Ok(output) => Ok(output),
                    Err(err) => Err(err),
                },
                GitAction::Pull => match git_access.pull() {
                    Ok(output) if output.is_empty() => Ok("Pull complete.".to_string()),
                    Ok(output) => Ok(output),
                    Err(err) => Err(err),
                },
                GitAction::PullPush => match git_access.pull() {
                    Ok(_) => match git_access.push() {
                        Ok(output) if output.is_empty() => {
                            Ok("Pull and push complete.".to_string())
                        }
                        Ok(output) => Ok(output),
                        Err(err) => Err(format!("Pull succeeded, but Push failed: {err}")),
                    },
                    Err(err) => Err(err),
                },
                GitAction::Publish(remote, branch) => match git_access.publish(&remote, &branch) {
                    Ok(output) if output.is_empty() => Ok("Publish complete.".to_string()),
                    Ok(output) => Ok(output),
                    Err(err) => Err(err),
                },
                GitAction::Fetch(remote) => {
                    let remote_name = remote.as_deref().unwrap_or("remote");
                    let progress_sender = sender.clone();
                    let _ = progress_sender.send(GitActionEvent::Progress(format!(
                        "Fetching {remote_name}..."
                    )));
                    let mut progress = move |progress| {
                        let _ = progress_sender.send(GitActionEvent::Progress(progress));
                    };
                    match git_access.fetch_with_progress(remote.as_deref(), &mut progress) {
                        Ok(output) if output.is_empty() => Ok(format!("Fetched {remote_name}.")),
                        Ok(output) => Ok(output),
                        Err(err) => Err(err),
                    }
                }
            };

            let _ = sender.send(GitActionEvent::Finished(message));
        }
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let state = state.clone();
        move || {
            let mut latest_progress = None;

            loop {
                match receiver.try_recv() {
                    Ok(GitActionEvent::Progress(progress)) => {
                        log::trace!("git action progress: {progress}");
                        latest_progress = Some(progress);
                    }
                    Ok(GitActionEvent::Finished(Ok(message))) => {
                        state.git_action_running.set(false);
                        state.content.clear_git_action_progress();
                        log::debug!("git action completed successfully: {message}");
                        if refresh_repo_metadata_on_success {
                            refresh_active_repo_metadata(&state, None);
                        }
                        if show_success_toast {
                            refresh(&state, Some(message));
                        } else {
                            refresh_without_toast(&state, Some(message));
                        }
                        return gtk::glib::ControlFlow::Break;
                    }
                    Ok(GitActionEvent::Finished(Err(err))) => {
                        state.git_action_running.set(false);
                        state.content.clear_git_action_progress();
                        log::debug!("git action failed: {err}");
                        if matches!(action, GitAction::Pull | GitAction::PullPush)
                            && is_local_changes_overwritten_error(&err)
                        {
                            state.content.update(&snapshot, None, false, false);
                            let files = parse_files_to_be_overwritten(&err);
                            log::info!(
                                "git pull blocked by local changes action={:?} overwritten_files={}",
                                action,
                                files.len()
                            );
                            show_local_changes_overwritten_dialog(
                                &state,
                                snapshot.clone(),
                                action.clone(),
                                git_access.clone(),
                                files,
                            );
                            return gtk::glib::ControlFlow::Break;
                        }
                        show_error_dialog(&state.window, "Git Action Failed", &err);
                        refresh(&state, None);
                        return gtk::glib::ControlFlow::Break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        state.git_action_running.set(false);
                        state.content.clear_git_action_progress();
                        show_error_dialog(
                            &state.window,
                            "Git Action Failed",
                            "Git action did not return a result.",
                        );
                        refresh(&state, None);
                        return gtk::glib::ControlFlow::Break;
                    }
                }
            }

            if let Some(progress) = latest_progress {
                state.content.set_git_action_progress(&progress);
            }

            gtk::glib::ControlFlow::Continue
        }
    });
}

fn show_local_changes_overwritten_dialog(
    state: &Rc<AppState>,
    snapshot: RepositorySnapshot,
    action: GitAction,
    git_access: Arc<dyn GitAccess>,
    files: Vec<String>,
) {
    let body = local_changes_overwritten_body(&action, &files);
    let dialog = adw::AlertDialog::builder()
        .heading("Local Changes Would Be Overwritten")
        .body(&body)
        .build();
    dialog.add_response("close", "Close");
    dialog.add_response("stash", "Stash changes and continue");
    dialog.set_default_response(Some("stash"));
    dialog.set_close_response("close");
    dialog.set_response_appearance("stash", adw::ResponseAppearance::Suggested);

    dialog.choose(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        move |response| {
            if response.as_str() != "stash" {
                return;
            }

            stash_changes_and_retry_git_action(
                &state,
                snapshot.clone(),
                action.clone(),
                git_access.clone(),
            );
        }
    });
}

fn local_changes_overwritten_body(action: &GitAction, files: &[String]) -> String {
    let action_name = match action {
        GitAction::PullPush => "pull remote changes before pushing",
        _ => "pull",
    };
    let mut body = format!("Unable to {action_name} when changes are present on your branch.");

    if !files.is_empty() {
        body.push_str(" The following files would be overwritten:");
        let visible_files = files.iter().take(12).collect::<Vec<_>>();
        for file in visible_files {
            body.push_str("\n  ");
            body.push_str(file);
        }
        if files.len() > 12 {
            body.push_str(&format!("\n  ... and {} more", files.len() - 12));
        }
    }

    body.push_str("\n\nYou can stash your changes now and recover them afterwards.");
    body
}

fn stash_changes_and_retry_git_action(
    state: &Rc<AppState>,
    snapshot: RepositorySnapshot,
    action: GitAction,
    git_access: Arc<dyn GitAccess>,
) {
    if state.git_action_running.get() {
        return;
    }

    let (sender, receiver) = mpsc::channel();
    state.git_action_running.set(true);
    state.content.clear_git_action_progress();
    state.content.update(&snapshot, None, true, false);
    state.content.set_git_action_progress("Stashing changes...");
    log::info!(
        "stashing local changes before retrying git action {:?}",
        action
    );

    thread::spawn(move || {
        let result = git_access.stash_changes().map(|output| {
            if output.is_empty() {
                "Changes stashed.".to_string()
            } else {
                output
            }
        });
        let _ = sender.send(GitActionEvent::Finished(result));
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let state = state.clone();
        move || match receiver.try_recv() {
            Ok(GitActionEvent::Finished(Ok(message))) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                log::info!("stash before retry completed: {message}");
                execute_git_action(&state, action.clone());
                gtk::glib::ControlFlow::Break
            }
            Ok(GitActionEvent::Finished(Err(err))) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                log::warn!("stash before retry failed: {err}");
                show_error_dialog(&state.window, "Stash Failed", &err);
                refresh(&state, None);
                gtk::glib::ControlFlow::Break
            }
            Ok(GitActionEvent::Progress(progress)) => {
                state.content.set_git_action_progress(&progress);
                gtk::glib::ControlFlow::Continue
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                state.git_action_running.set(false);
                state.content.clear_git_action_progress();
                state.content.update(&snapshot, None, false, false);
                show_error_dialog(
                    &state.window,
                    "Stash Failed",
                    "Stash worker stopped unexpectedly.",
                );
                refresh(&state, None);
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn is_local_changes_overwritten_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let would_overwrite = lower.contains("would be overwritten")
        && (lower.contains("local changes")
            || lower.contains("untracked working tree files")
            || lower.contains("files would be overwritten"));
    let rebase_dirty = lower.contains("cannot pull with rebase")
        && (lower.contains("unstaged changes")
            || lower.contains("uncommitted changes")
            || lower.contains("please commit or stash"));
    let merge_dirty = lower.contains("commit your changes or stash them")
        && (lower.contains("merge") || lower.contains("pull"));

    would_overwrite || rebase_dirty || merge_dirty
}

fn parse_files_to_be_overwritten(message: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut in_files_list = false;

    for line in message.lines() {
        if in_files_list {
            if line.starts_with('\t') || line.starts_with("    ") {
                let file = line.trim();
                if !file.is_empty() {
                    files.push(file.to_string());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            break;
        }

        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("error:")
            && lower.contains("would be overwritten")
            && lower.trim_end().ends_with(':')
        {
            in_files_list = true;
        }
    }

    files.sort();
    files.dedup();
    files
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
