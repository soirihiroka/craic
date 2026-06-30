use super::dialogs::show_error_dialog;
use super::git_actions::{GitAction, execute_git_action};
use super::preferences::show_preferences_window;
use super::{
    AGENT_SESSION_NOTIFICATION_ACTION, AppState, activate_page, active_workspace_from_config,
    agent_session_notification_id, apply_workspace_color, dispatch_page_command,
    pages::PageCommand, refresh, refresh_active_page, refresh_active_repo_metadata,
};
use crate::config::{ConfiguredWorkspace, WorkspaceProvider};
use adw::prelude::*;
use gtk::{gdk, gio};
use std::rc::Rc;

pub(super) fn connect_shell_actions(state: &Rc<AppState>) {
    connect_app_actions(state);
    connect_open_search_shortcut(state);
    connect_repository_picker(state);
    connect_sidebar_mode_buttons(state);
}

fn connect_app_actions(state: &Rc<AppState>) {
    if let Some(app) = state.window.application() {
        let open_repo =
            gio::SimpleAction::new("open_repo", Some(gtk::glib::VariantTy::new("s").unwrap()));
        open_repo.connect_activate({
            let state = state.clone();
            move |_, parameter| {
                if let Some(val) = parameter.and_then(|v| v.str()) {
                    let workspace = crate::workspace::workspace_from_selection_id(val);
                    set_active_workspace(&state, workspace);
                    refresh(&state, None);
                }
            }
        });
        app.add_action(&open_repo);

        let pull_action = gio::SimpleAction::new("pull", None);
        pull_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                execute_git_action(&state, GitAction::Pull);
            }
        });
        app.add_action(&pull_action);
        app.set_accels_for_action("app.pull", &["<Control>p"]);

        let push_action = gio::SimpleAction::new("push", None);
        push_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                execute_git_action(&state, GitAction::Push);
            }
        });
        app.add_action(&push_action);
        app.set_accels_for_action("app.push", &["<Control>u"]);

        let refresh_action = gio::SimpleAction::new("refresh", None);
        refresh_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                refresh(&state, Some("Workspace status refreshed.".to_string()));
            }
        });
        app.add_action(&refresh_action);
        app.set_accels_for_action("app.refresh", &["<Control>r"]);

        let refresh_page_action = gio::SimpleAction::new("refresh_page", None);
        refresh_page_action.connect_activate({
            let state = state.clone();
            move |_, _| {
                refresh_active_page(&state);
            }
        });
        app.add_action(&refresh_page_action);
        app.set_accels_for_action("app.refresh_page", &["F5"]);

        let open_agent_session = gio::SimpleAction::new(
            AGENT_SESSION_NOTIFICATION_ACTION,
            Some(gtk::glib::VariantTy::new("t").unwrap()),
        );
        open_agent_session.connect_activate({
            let state = state.clone();
            move |_, parameter| {
                let Some(session_id) = parameter.and_then(|value| value.get::<u64>()) else {
                    log::warn!("agent notification activated without valid session target");
                    return;
                };

                log::info!("agent notification activated session_id={session_id}");
                state.window.present();
                let result =
                    dispatch_page_command(&state, PageCommand::OpenAgentSession(session_id));
                log::debug!(
                    "agent notification dispatch completed session_id={} handled={}",
                    session_id,
                    result != super::pages::PageCommandResult::Ignored
                );

                if let Some(app) = state.window.application() {
                    app.withdraw_notification(&agent_session_notification_id(session_id));
                }
            }
        });
        app.add_action(&open_agent_session);

        let preferences = gio::SimpleAction::new("preferences", None);
        preferences.connect_activate({
            let state = state.clone();
            move |_, _| {
                let system = state.system_ref.borrow().clone();
                let workspace = state.workspace_ref.borrow().clone();
                show_preferences_window(
                    &state.window,
                    state.providers.git(&system.id, &workspace),
                    state.providers.github(&system.id, &workspace),
                );
            }
        });
        app.add_action(&preferences);
        app.set_accels_for_action("app.preferences", &["<Control>comma"]);
    }
}

fn connect_open_search_shortcut(state: &Rc<AppState>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let state = state.clone();

        move |_, key, _, modifiers| {
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            if !(ctrl && !alt && matches!(key, gdk::Key::f | gdk::Key::F)) {
                return gtk::glib::Propagation::Proceed;
            }

            let index = state.active_page.get();
            let Some(page) = state.pages.get(index) else {
                return gtk::glib::Propagation::Proceed;
            };

            let handled = if state.page_host.left_hovered() {
                page.toggle_left_search()
            } else if state.page_host.right_hovered() {
                page.toggle_right_search()
            } else {
                false
            };

            if handled {
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        }
    });
    state.window.add_controller(keys);
}

fn connect_repository_picker(state: &Rc<AppState>) {
    state.sidebar.repository_picker.connect_item_activated({
        let state = state.clone();
        move |id| {
            let workspace = crate::workspace::workspace_from_selection_id(&id);
            set_active_workspace(&state, workspace);
            refresh(&state, None);
        }
    });

    state.sidebar.repository_picker.connect_add_clicked({
        let state = state.clone();
        move || open_repository_folder_dialog(&state)
    });

    state.sidebar.repository_picker.connect_opened({
        let state = state.clone();
        move || state.sidebar.load_repos_async()
    });
}

fn connect_sidebar_mode_buttons(state: &Rc<AppState>) {
    for (index, button) in state.sidebar.mode_switcher.buttons.iter().enumerate() {
        button.connect_toggled({
            let state = state.clone();
            move |button| {
                if button.is_active() {
                    activate_page(&state, index);
                }
            }
        });
    }
}

fn set_active_workspace(state: &Rc<AppState>, workspace: ConfiguredWorkspace) {
    let active = active_workspace_from_config(&state.providers, &workspace);
    let item_id = workspace.selection_id();
    *state.repo_path.borrow_mut() = active.repo_path.clone();
    state.system_ref.replace(active.system_ref);
    state.workspace_ref.replace(active.workspace_ref);
    apply_workspace_color(state);
    crate::config::save_last_workspace(&workspace);
    refresh_active_repo_metadata(state, Some(item_id));
}

fn open_repository_folder_dialog(state: &Rc<AppState>) {
    let dialog = gtk::FileDialog::builder()
        .title("Open Workspace")
        .modal(true)
        .build();
    dialog.select_folder(Some(&state.window), None::<&gio::Cancellable>, {
        let state = state.clone();
        move |result| match result {
            Ok(folder) => match folder.path() {
                Some(path) => {
                    set_active_workspace(
                        &state,
                        ConfiguredWorkspace {
                            path: path.to_string_lossy().to_string(),
                            provider: WorkspaceProvider::Local,
                            display_name: None,
                            color: None,
                        },
                    );
                    refresh(&state, None);
                }
                None => show_error_dialog(
                    &state.window,
                    "Open Workspace Failed",
                    "The selected folder does not have a local path.",
                ),
            },
            Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
            Err(err) => show_error_dialog(&state.window, "Open Workspace Failed", &err.to_string()),
        }
    });
}
