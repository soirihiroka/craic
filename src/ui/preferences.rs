use super::dialogs::show_error_dialog;
use crate::agent_provider;
use crate::config::{self, FontSizes};
use crate::git::GitSettings;
use crate::github::GitHubAuthAccount;
use crate::system::capabilities::git::GitAccess;
use crate::system::capabilities::github::GitHubAccess;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

pub(super) fn show_preferences_window(
    parent: &adw::ApplicationWindow,
    git_access: Option<Arc<dyn GitAccess>>,
    github_access: Option<Arc<dyn GitHubAccess>>,
) {
    let app_config = config::load();
    let git_settings = git_access.as_ref().map(|access| access.settings());
    let prefs_window = adw::PreferencesDialog::builder()
        .title("Preferences")
        .content_width(550)
        .content_height(450)
        .build();

    let general_page = adw::PreferencesPage::new();
    general_page.set_title("General");
    general_page.set_icon_name(Some("preferences-system-symbolic"));

    let git_group = adw::PreferencesGroup::new();
    git_group.set_title("Git Configuration");
    git_group.set_description(Some("Configure global Git default options"));

    let branch_row = adw::EntryRow::builder()
        .title("Default Branch Name")
        .build();
    branch_row.set_text("main");
    git_group.add(&branch_row);

    let pull_row = adw::ActionRow::builder()
        .title("Default Pull Strategy")
        .subtitle("Prefer rebase when pulling remote changes")
        .build();
    let pull_switch = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    pull_row.add_suffix(&pull_switch);
    git_group.add(&pull_row);

    let fetch_row = adw::ActionRow::builder()
        .title("Background Fetch")
        .subtitle("Automatically fetch remote changes periodically")
        .build();
    let fetch_switch = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    fetch_row.add_suffix(&fetch_switch);
    git_group.add(&fetch_row);

    general_page.add(&git_group);

    let agent_page = adw::PreferencesPage::new();
    agent_page.set_title("AI");
    agent_page.set_icon_name(Some("brain-augemnted-symbolic"));

    let ai_group = adw::PreferencesGroup::new();
    ai_group.set_title("Commit Message");
    ai_group.set_description(Some("Provider for the auto generated commit message"));

    let providers = agent_provider::registered_providers();
    let provider_labels = providers
        .iter()
        .map(|provider| provider.label())
        .collect::<Vec<_>>();
    let provider_model = gtk::StringList::new(&provider_labels);
    let initial_provider_id = selected_registered_provider_id(&app_config.commit_message_provider);
    let provider_row = adw::ComboRow::builder()
        .title("Provider")
        .model(&provider_model)
        .selected(provider_index(initial_provider_id))
        .build();
    ai_group.add(&provider_row);

    let initial_model_label = provider_default_model_label(initial_provider_id);
    let initial_model_model = gtk::StringList::new(&[initial_model_label.as_str()]);
    let model_row = adw::ComboRow::builder()
        .title("Model")
        .model(&initial_model_model)
        .selected(0)
        .build();
    let model_loading_spinner = adw::Spinner::new();
    model_loading_spinner.set_size_request(16, 16);
    model_loading_spinner.set_valign(gtk::Align::Center);
    model_loading_spinner.set_visible(false);
    model_row.add_suffix(&model_loading_spinner);
    ai_group.add(&model_row);

    let current_provider = Rc::new(RefCell::new(initial_provider_id.to_string()));
    let current_model = Rc::new(RefCell::new(app_config.commit_message_model.clone()));
    let model_choices = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
    let model_request_id = Rc::new(Cell::new(0u64));
    let suppress_model_save = Rc::new(Cell::new(false));

    load_commit_models_for_provider(
        initial_provider_id,
        &model_row,
        &model_loading_spinner,
        &model_choices,
        &current_model,
        &model_request_id,
        &suppress_model_save,
    );

    provider_row.connect_selected_notify({
        let model_row = model_row.clone();
        let model_loading_spinner = model_loading_spinner.clone();
        let current_provider = current_provider.clone();
        let current_model = current_model.clone();
        let model_choices = model_choices.clone();
        let model_request_id = model_request_id.clone();
        let suppress_model_save = suppress_model_save.clone();
        let providers = providers;

        move |row| {
            let provider = providers
                .get(row.selected() as usize)
                .copied()
                .unwrap_or_else(agent_provider::default_provider);
            let provider_id = provider.id().to_string();
            current_provider.replace(provider_id.clone());
            current_model.replace(None);
            config::save_commit_message_provider(&provider_id);
            load_commit_models_for_provider(
                &provider_id,
                &model_row,
                &model_loading_spinner,
                &model_choices,
                &current_model,
                &model_request_id,
                &suppress_model_save,
            );
        }
    });

    model_row.connect_selected_notify({
        let current_provider = current_provider.clone();
        let current_model = current_model.clone();
        let model_choices = model_choices.clone();
        let suppress_model_save = suppress_model_save.clone();

        move |row| {
            if suppress_model_save.get() {
                return;
            }

            let model = model_choices
                .borrow()
                .get(row.selected() as usize)
                .cloned()
                .flatten();
            current_model.replace(model.clone());
            let provider_id = current_provider.borrow().clone();
            config::save_commit_message_model(&provider_id, model.as_deref());
        }
    });

    agent_page.add(&ai_group);

    let smart_group = adw::PreferencesGroup::new();
    smart_group.set_title("Chat Summary");
    smart_group.set_description(Some(
        "Provider and model used to generate smart summaries for chat sessions",
    ));
    for shell_provider_id in config::smart_feature_shell_providers() {
        add_smart_feature_rows(&smart_group, shell_provider_id, &app_config, providers);
    }
    agent_page.add(&smart_group);

    let font_group = adw::PreferencesGroup::new();
    font_group.set_title("Font Sizes");

    let shell_font_row = font_size_row("Shell Font Size", app_config.font_sizes.shell);
    let editor_font_row = font_size_row("Text Editor Font Size", app_config.font_sizes.editor);
    let diff_font_row = font_size_row("Diff Font Size", app_config.font_sizes.diff);
    font_group.add(&shell_font_row);
    font_group.add(&editor_font_row);
    font_group.add(&diff_font_row);

    let save_font_sizes = Rc::new({
        let shell_font_row = shell_font_row.clone();
        let editor_font_row = editor_font_row.clone();
        let diff_font_row = diff_font_row.clone();

        move || {
            config::save_font_sizes(FontSizes {
                shell: shell_font_row.value(),
                editor: editor_font_row.value(),
                diff: diff_font_row.value(),
            });
        }
    });

    shell_font_row.connect_value_notify({
        let save_font_sizes = save_font_sizes.clone();
        move |_| save_font_sizes()
    });
    editor_font_row.connect_value_notify({
        let save_font_sizes = save_font_sizes.clone();
        move |_| save_font_sizes()
    });
    diff_font_row.connect_value_notify({
        let save_font_sizes = save_font_sizes.clone();
        move |_| save_font_sizes()
    });

    general_page.add(&font_group);

    if let (Some(git_access), Some(settings)) = (git_access, git_settings) {
        let workspace_page = adw::PreferencesPage::new();
        workspace_page.set_title("Workspace");
        workspace_page.set_icon_name(Some("git-symbolic"));

        let profile_group = adw::PreferencesGroup::new();
        profile_group.set_title("Git Author Profile");
        profile_group.set_description(Some("Settings for the current workspace"));

        let use_global_row = adw::ActionRow::builder()
            .title("Use Global User")
            .subtitle("Use the global Git user.name and user.email for this workspace")
            .build();
        let use_global_switch = gtk::Switch::builder()
            .active(settings.use_global_user)
            .valign(gtk::Align::Center)
            .build();
        use_global_row.add_suffix(&use_global_switch);
        profile_group.add(&use_global_row);

        let name_row = adw::EntryRow::builder().title("Author Name").build();
        name_row.set_text(
            settings
                .local_user_name
                .as_deref()
                .or(settings.global_user_name.as_deref())
                .unwrap_or(""),
        );
        name_row.set_sensitive(!settings.use_global_user);
        profile_group.add(&name_row);

        let email_row = adw::EntryRow::builder().title("Author Email").build();
        email_row.set_text(
            settings
                .local_user_email
                .as_deref()
                .or(settings.global_user_email.as_deref())
                .unwrap_or(""),
        );
        email_row.set_sensitive(!settings.use_global_user);
        profile_group.add(&email_row);

        workspace_page.add(&profile_group);

        let privacy_group = adw::PreferencesGroup::new();
        privacy_group.set_title("Commit Privacy");
        privacy_group.set_description(Some("Optional settings applied when Craic creates commits"));

        let timezone_row = adw::EntryRow::builder().title("Commit Timezone").build();
        timezone_row.set_text(settings.commit_timezone.as_deref().unwrap_or(""));
        timezone_row.set_tooltip_text(Some(
            "Use +0000, -0500, or +09:30. Leave empty to use +0000 unless system timezone is enabled.",
        ));
        privacy_group.add(&timezone_row);

        let use_system_timezone_row = adw::ActionRow::builder()
            .title("Use System Timezone")
            .subtitle("Use Git's default timezone when no commit timezone is set")
            .build();
        let use_system_timezone_switch = gtk::Switch::builder()
            .active(settings.use_system_timezone)
            .valign(gtk::Align::Center)
            .build();
        use_system_timezone_row.add_suffix(&use_system_timezone_switch);
        privacy_group.add(&use_system_timezone_row);
        let remote_owner_warning_row = adw::ActionRow::builder()
            .title("Show Remote Owner Warning")
            .subtitle("Warn when the Git author does not match the remote owner.")
            .build();
        let remote_owner_warning_switch = gtk::Switch::builder()
            .active(settings.warn_if_remote_owner_mismatch)
            .valign(gtk::Align::Center)
            .build();
        remote_owner_warning_row.add_suffix(&remote_owner_warning_switch);
        privacy_group.add(&remote_owner_warning_row);

        workspace_page.add(&privacy_group);

        let github_group = adw::PreferencesGroup::new();
        github_group.set_title("GitHub");
        github_group.set_description(Some(
            "Local GitHub CLI account preference for this workspace",
        ));
        let mut initial_github_account_choices = vec![None::<GitHubAuthAccount>];
        let mut initial_github_account_labels = vec!["Use active gh account".to_string()];
        if let Some(account) = settings.github_auth_account.clone() {
            initial_github_account_labels.push(github_auth_account_label(&account));
            initial_github_account_choices.push(Some(account));
        }
        let initial_github_account_label_refs = initial_github_account_labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let github_account_choices = Rc::new(RefCell::new(initial_github_account_choices));
        let github_account_model = gtk::StringList::new(&initial_github_account_label_refs);
        let github_account_row = adw::ComboRow::builder()
            .title("Authenticated Account")
            .model(&github_account_model)
            .selected(if settings.github_auth_account.is_some() {
                1
            } else {
                0
            })
            .build();
        let github_account_spinner = adw::Spinner::new();
        github_account_spinner.set_size_request(16, 16);
        github_account_spinner.set_valign(gtk::Align::Center);
        github_account_spinner.set_visible(github_access.is_some());
        github_account_row.add_suffix(&github_account_spinner);
        if let Some(github_access) = github_access {
            load_github_auth_account_choices(
                github_access,
                settings.github_auth_account.as_ref(),
                &github_account_row,
                &github_account_spinner,
                &github_account_choices,
            );
        } else {
            github_account_row.set_sensitive(false);
            github_account_row.set_subtitle("GitHub CLI access is unavailable for this workspace.");
        }
        github_group.add(&github_account_row);
        workspace_page.add(&github_group);

        let save_workspace_settings = Rc::new({
            let parent = parent.clone();
            let git_access = git_access.clone();
            let base_settings = settings.clone();
            let use_global_switch = use_global_switch.clone();
            let name_row = name_row.clone();
            let email_row = email_row.clone();
            let timezone_row = timezone_row.clone();
            let use_system_timezone_switch = use_system_timezone_switch.clone();
            let remote_owner_warning_switch = remote_owner_warning_switch.clone();
            let github_account_row = github_account_row.clone();
            let github_account_choices = github_account_choices.clone();

            move || {
                let github_auth_account = github_account_choices
                    .borrow()
                    .get(github_account_row.selected() as usize)
                    .cloned()
                    .flatten();
                let next_settings = GitSettings {
                    global_user_name: base_settings.global_user_name.clone(),
                    global_user_email: base_settings.global_user_email.clone(),
                    local_user_name: text_option(&name_row.text()),
                    local_user_email: text_option(&email_row.text()),
                    use_global_user: use_global_switch.is_active(),
                    commit_timezone: text_option(&timezone_row.text()),
                    warn_if_remote_owner_mismatch: remote_owner_warning_switch.is_active(),
                    use_system_timezone: use_system_timezone_switch.is_active(),
                    github_auth_account,
                };
                match git_access.save_settings(&next_settings) {
                    Ok(()) => {}
                    Err(err) => show_error_dialog(&parent, "Failed to Save Preferences", &err),
                }
            }
        });

        use_global_switch.connect_active_notify({
            let name_row = name_row.clone();
            let email_row = email_row.clone();
            let save_workspace_settings = save_workspace_settings.clone();

            move |switch| {
                let sensitive = !switch.is_active();
                name_row.set_sensitive(sensitive);
                email_row.set_sensitive(sensitive);
                save_workspace_settings();
            }
        });

        name_row.connect_changed({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });

        email_row.connect_changed({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });

        timezone_row.connect_changed({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });

        use_system_timezone_switch.connect_active_notify({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });
        remote_owner_warning_switch.connect_active_notify({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });
        github_account_row.connect_selected_notify({
            let save_workspace_settings = save_workspace_settings.clone();

            move |_| save_workspace_settings()
        });

        prefs_window.add(&workspace_page);
    }

    prefs_window.add(&general_page);
    prefs_window.add(&agent_page);

    prefs_window.present(Some(parent));
}

fn text_option(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn load_github_auth_account_choices(
    github_access: Arc<dyn GitHubAccess>,
    selected_account: Option<&GitHubAuthAccount>,
    row: &adw::ComboRow,
    spinner: &adw::Spinner,
    choices: &Rc<RefCell<Vec<Option<GitHubAuthAccount>>>>,
) {
    let selected_account = selected_account.cloned();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = github_access.authenticated_accounts();
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let row = row.clone();
        let spinner = spinner.clone();
        let choices = choices.clone();

        move || match receiver.try_recv() {
            Ok(Ok(accounts)) => {
                spinner.set_visible(false);
                set_github_auth_account_choices(
                    &row,
                    &choices,
                    accounts,
                    selected_account.as_ref(),
                );
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                spinner.set_visible(false);
                row.set_subtitle(&err);
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                spinner.set_visible(false);
                row.set_subtitle("Loading GitHub accounts stopped unexpectedly.");
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn set_github_auth_account_choices(
    row: &adw::ComboRow,
    choices: &Rc<RefCell<Vec<Option<GitHubAuthAccount>>>>,
    mut accounts: Vec<GitHubAuthAccount>,
    selected_account: Option<&GitHubAuthAccount>,
) {
    accounts.sort_by(|left, right| {
        left.host
            .cmp(&right.host)
            .then_with(|| left.login.cmp(&right.login))
    });

    if let Some(selected_account) = selected_account {
        let is_listed = accounts.iter().any(|account| account == selected_account);
        if !is_listed {
            accounts.push(selected_account.clone());
        }
    }

    let mut next_choices = vec![None];
    next_choices.extend(accounts.iter().cloned().map(Some));
    let mut labels = vec!["Use active gh account".to_string()];
    labels.extend(accounts.iter().map(github_auth_account_label));
    let label_refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let model = gtk::StringList::new(&label_refs);
    let selected_index = selected_account
        .and_then(|selected| {
            next_choices
                .iter()
                .position(|choice| choice.as_ref() == Some(selected))
        })
        .unwrap_or_default();

    choices.replace(next_choices);
    row.set_model(Some(&model));
    row.set_selected(selected_index as u32);
    row.set_sensitive(true);
}

fn github_auth_account_label(account: &GitHubAuthAccount) -> String {
    if account.host.eq_ignore_ascii_case("github.com") {
        account.login.clone()
    } else {
        format!("{} on {}", account.login, account.host)
    }
}

#[derive(Clone, Debug)]
struct ModelChoice {
    id: Option<String>,
    label: String,
}

fn load_commit_models_for_provider(
    provider_id: &str,
    model_row: &adw::ComboRow,
    model_loading_spinner: &adw::Spinner,
    model_choices: &Rc<RefCell<Vec<Option<String>>>>,
    current_model: &Rc<RefCell<Option<String>>>,
    model_request_id: &Rc<Cell<u64>>,
    suppress_model_save: &Rc<Cell<bool>>,
) {
    let request_id = model_request_id.get().wrapping_add(1);
    model_request_id.set(request_id);
    let provider_id = provider_id.to_string();
    let selected_model = current_model.borrow().clone();
    set_commit_model_choices(
        model_row,
        model_choices,
        suppress_model_save,
        model_choices_for(&provider_id, Vec::new(), selected_model.as_deref()),
        selected_model.as_deref(),
    );
    model_loading_spinner.set_visible(true);

    let (sender, receiver) = mpsc::channel();
    let provider_id_for_thread = provider_id.clone();
    thread::spawn(move || {
        let result = agent_provider::model_options(&provider_id_for_thread);
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let model_row = model_row.clone();
        let model_loading_spinner = model_loading_spinner.clone();
        let model_choices = model_choices.clone();
        let current_model = current_model.clone();
        let model_request_id = model_request_id.clone();
        let suppress_model_save = suppress_model_save.clone();

        move || {
            if model_request_id.get() != request_id {
                return gtk::glib::ControlFlow::Break;
            }

            match receiver.try_recv() {
                Ok(Ok(models)) => {
                    model_loading_spinner.set_visible(false);
                    let selected_model = current_model.borrow().clone();
                    set_commit_model_choices(
                        &model_row,
                        &model_choices,
                        &suppress_model_save,
                        model_choices_for(&provider_id, models, selected_model.as_deref()),
                        selected_model.as_deref(),
                    );
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(_err)) => {
                    model_loading_spinner.set_visible(false);
                    let selected_model = current_model.borrow().clone();
                    set_commit_model_choices(
                        &model_row,
                        &model_choices,
                        &suppress_model_save,
                        model_choices_for(&provider_id, Vec::new(), selected_model.as_deref()),
                        selected_model.as_deref(),
                    );
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    model_loading_spinner.set_visible(false);
                    let selected_model = current_model.borrow().clone();
                    set_commit_model_choices(
                        &model_row,
                        &model_choices,
                        &suppress_model_save,
                        model_choices_for(&provider_id, Vec::new(), selected_model.as_deref()),
                        selected_model.as_deref(),
                    );
                    gtk::glib::ControlFlow::Break
                }
            }
        }
    });
}

fn add_smart_feature_rows(
    group: &adw::PreferencesGroup,
    shell_provider_id: &str,
    app_config: &config::AppConfig,
    providers: &'static [&'static dyn agent_provider::AgentProvider],
) {
    let saved = app_config
        .smart_features
        .get(shell_provider_id)
        .cloned()
        .unwrap_or_else(|| config::SmartFeatureConfig {
            provider: shell_provider_id.to_string(),
            model: None,
        });
    let initial_provider_id = selected_registered_provider_id(&saved.provider);
    let provider_labels = providers
        .iter()
        .map(|provider| provider.label())
        .collect::<Vec<_>>();
    let provider_model = gtk::StringList::new(&provider_labels);
    let title = format!(
        "{} Summary Provider",
        smart_feature_label(shell_provider_id)
    );
    let provider_row = adw::ComboRow::builder()
        .title(&title)
        .model(&provider_model)
        .selected(provider_index(initial_provider_id))
        .build();
    group.add(&provider_row);

    let initial_model_label = provider_default_model_label(initial_provider_id);
    let initial_model_model = gtk::StringList::new(&[initial_model_label.as_str()]);
    let model_title = format!("{} Summary Model", smart_feature_label(shell_provider_id));
    let model_row = adw::ComboRow::builder()
        .title(&model_title)
        .model(&initial_model_model)
        .selected(0)
        .build();
    let model_loading_spinner = adw::Spinner::new();
    model_loading_spinner.set_size_request(16, 16);
    model_loading_spinner.set_valign(gtk::Align::Center);
    model_loading_spinner.set_visible(false);
    model_row.add_suffix(&model_loading_spinner);
    group.add(&model_row);

    let current_provider = Rc::new(RefCell::new(initial_provider_id.to_string()));
    let current_model = Rc::new(RefCell::new(saved.model.clone()));
    let model_choices = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
    let model_request_id = Rc::new(Cell::new(0u64));
    let suppress_model_save = Rc::new(Cell::new(false));

    load_commit_models_for_provider(
        initial_provider_id,
        &model_row,
        &model_loading_spinner,
        &model_choices,
        &current_model,
        &model_request_id,
        &suppress_model_save,
    );

    provider_row.connect_selected_notify({
        let shell_provider_id = shell_provider_id.to_string();
        let model_row = model_row.clone();
        let model_loading_spinner = model_loading_spinner.clone();
        let current_provider = current_provider.clone();
        let current_model = current_model.clone();
        let model_choices = model_choices.clone();
        let model_request_id = model_request_id.clone();
        let suppress_model_save = suppress_model_save.clone();

        move |row| {
            let provider = providers
                .get(row.selected() as usize)
                .copied()
                .unwrap_or_else(agent_provider::default_provider);
            let provider_id = provider.id().to_string();
            current_provider.replace(provider_id.clone());
            current_model.replace(None);
            config::save_smart_feature_provider(&shell_provider_id, &provider_id);
            load_commit_models_for_provider(
                &provider_id,
                &model_row,
                &model_loading_spinner,
                &model_choices,
                &current_model,
                &model_request_id,
                &suppress_model_save,
            );
        }
    });

    model_row.connect_selected_notify({
        let shell_provider_id = shell_provider_id.to_string();
        let current_provider = current_provider.clone();
        let current_model = current_model.clone();
        let model_choices = model_choices.clone();
        let suppress_model_save = suppress_model_save.clone();

        move |row| {
            if suppress_model_save.get() {
                return;
            }

            let model = model_choices
                .borrow()
                .get(row.selected() as usize)
                .cloned()
                .flatten();
            current_model.replace(model.clone());
            let provider_id = current_provider.borrow().clone();
            config::save_smart_feature_model(&shell_provider_id, &provider_id, model.as_deref());
        }
    });
}

fn smart_feature_label(shell_provider_id: &str) -> &'static str {
    match shell_provider_id {
        "codex" => "Codex",
        "agy" => "AGY",
        "opencode" => "OpenCode",
        _ => "Agent",
    }
}

fn model_choices_for(
    provider_id: &str,
    models: Vec<agent_provider::ModelOption>,
    selected_model: Option<&str>,
) -> Vec<ModelChoice> {
    let mut choices = vec![ModelChoice {
        id: None,
        label: provider_default_model_label(provider_id),
    }];
    choices.extend(models.into_iter().map(|model| {
        let label = if model.label == model.id {
            model.id.clone()
        } else {
            format!("{} ({})", model.label, model.id)
        };
        ModelChoice {
            id: Some(model.id),
            label,
        }
    }));

    if let Some(selected_model) = selected_model {
        let is_listed = choices
            .iter()
            .any(|choice| choice.id.as_deref() == Some(selected_model));
        if !is_listed {
            choices.push(ModelChoice {
                id: Some(selected_model.to_string()),
                label: format!("{selected_model} (saved)"),
            });
        }
    }

    choices
}

fn set_commit_model_choices(
    row: &adw::ComboRow,
    model_choices: &Rc<RefCell<Vec<Option<String>>>>,
    suppress_model_save: &Rc<Cell<bool>>,
    choices: Vec<ModelChoice>,
    selected_model: Option<&str>,
) {
    let selected_index = selected_model
        .and_then(|selected_model| {
            choices
                .iter()
                .position(|choice| choice.id.as_deref() == Some(selected_model))
        })
        .unwrap_or_default();
    let labels = choices
        .iter()
        .map(|choice| choice.label.as_str())
        .collect::<Vec<_>>();
    let model = gtk::StringList::new(&labels);

    model_choices.replace(choices.iter().map(|choice| choice.id.clone()).collect());
    suppress_model_save.set(true);
    row.set_model(Some(&model));
    row.set_selected(selected_index as u32);
    suppress_model_save.set(false);
}

fn selected_registered_provider_id(configured_provider_id: &str) -> &'static str {
    agent_provider::find_provider(configured_provider_id)
        .unwrap_or_else(agent_provider::default_provider)
        .id()
}

fn provider_index(provider_id: &str) -> u32 {
    agent_provider::registered_providers()
        .iter()
        .position(|provider| provider.id() == provider_id)
        .unwrap_or_default() as u32
}

fn provider_default_model_label(provider_id: &str) -> String {
    agent_provider::find_provider(provider_id)
        .unwrap_or_else(agent_provider::default_provider)
        .default_model_label()
}

fn font_size_row(title: &str, value: f64) -> adw::SpinRow {
    let adjustment = gtk::Adjustment::new(
        value,
        config::MIN_FONT_SIZE,
        config::MAX_FONT_SIZE,
        1.0,
        2.0,
        0.0,
    );
    adw::SpinRow::builder()
        .title(title)
        .adjustment(&adjustment)
        .digits(0)
        .numeric(true)
        .build()
}
