pub(super) mod binary_preview;
mod canvas_overshoot;
pub(super) mod code_editor;
pub(super) mod diff_canvas;
mod diff_layout;
pub(super) mod diff_view;
pub(super) mod folder_view;
pub(super) mod pdf_preview;

use super::{file_type, widgets};
use crate::git::{self, QuickActionConfig, RepositorySnapshot};
use crate::github::PullRequestInfo;
use crate::quick_action::{self, RunItem, RunTargetsSignature};
use crate::system::WorkspacePath;
use crate::system::capabilities::{
    open::{DesktopOpenAccess, DesktopOpenActivation},
    shell::ShellAccess,
    terminal_link::{TerminalLinkAccess, TerminalLinkTarget},
};
use crate::terminal;
use crate::ui::components::tabbed_picker::{
    TabbedPicker, TabbedPickerGroup, TabbedPickerItem, TabbedPickerStatus, TabbedPickerTab,
};
use crate::ui::pages::{PageCommand, PageCommandResult};
use adw::prelude::*;
use craic_diff_ui::{Element, PartialEqRenderState};
use gtk::{gio, pango};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

const TERMINAL_CONFLICTING_ACCELS: &[(&str, &[&str])] = &[
    ("app.pull", &["<Control>p"]),
    ("app.push", &["<Control>u"]),
    ("app.refresh", &["<Control>r"]),
    ("app.refresh_page", &["F5"]),
    ("app.preferences", &["<Control>comma"]),
    ("app.shortcuts", &["<Control>question"]),
    ("app.about", &["F1"]),
];
static QUICK_ACTION_CSS_INSTALLED: AtomicBool = AtomicBool::new(false);

pub struct ContentPane {
    pub root: adw::ToolbarView,
    pub toast_overlay: adw::ToastOverlay,
    pub push_button: gtk::Button,
    pub terminal_toggle_button: gtk::ToggleButton,
    pub branch_picker: TabbedPicker,
    header: adw::HeaderBar,
    quick_actions_box: gtk::Box,
    quick_action_add_button: gtk::Button,
    quick_actions: Rc<RefCell<Vec<Rc<QuickActionButton>>>>,
    quick_action_next_id: Rc<Cell<u64>>,
    quick_action_config_repo: Rc<RefCell<Option<PathBuf>>>,
    quick_action_runner: Rc<RefCell<Option<Rc<dyn Fn(RunItem)>>>>,
    quick_action_state_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    run_targets: Rc<RefCell<Vec<RunItem>>>,
    run_targets_signature: RefCell<Option<RunTargetsSignature>>,
    terminal: terminal::TerminalPanel,
    push_icon: gtk::Image,
    push_spinner: adw::Spinner,
    push_label: gtk::Label,
    push_ahead_box: gtk::Box,
    push_behind_box: gtk::Box,
    push_ahead_label: gtk::Label,
    push_behind_label: gtk::Label,
    git_action_progress: RefCell<Option<String>>,
    page_slot: gtk::Box,
}

pub trait RepositoryActionContext: Clone {
    fn local_workspace_path(&self) -> Option<PathBuf>;
    fn workspace_root(&self) -> WorkspacePath;
    fn desktop_opener(&self) -> Option<Arc<dyn DesktopOpenAccess>>;
    fn terminal_links(&self) -> Option<Arc<dyn TerminalLinkAccess>>;
    fn shell(&self) -> Option<Arc<dyn ShellAccess>>;
    fn window(&self) -> adw::ApplicationWindow;
    fn refresh(&self, message: Option<String>);
    fn show_toast(&self, message: &str);
    fn run_git_action(&self);
    fn dispatch_command(&self, command: PageCommand) -> PageCommandResult;
}

pub fn build(_menu: &gio::Menu, snapshot: Option<&RepositorySnapshot>) -> ContentPane {
    let branch_picker = TabbedPicker::new(
        "Search branches",
        "New branch",
        snapshot
            .map(|snapshot| snapshot.branch.as_str())
            .unwrap_or("Branch"),
        "branch-symbolic",
        "Current branch",
        snapshot
            .map(branch_picker_tabs)
            .unwrap_or_else(empty_branch_tabs),
    );
    let push_icon = gtk::Image::from_icon_name("view-refresh-symbolic");
    let push_spinner = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .visible(false)
        .build();
    let push_label = gtk::Label::new(Some("Fetch remote"));
    push_label.set_ellipsize(pango::EllipsizeMode::End);
    push_label.set_max_width_chars(28);
    let push_ahead_icon = gtk::Image::from_icon_name("go-up-symbolic");
    let push_ahead_label = gtk::Label::new(None);
    let push_ahead_box = git_count_badge(&push_ahead_icon, &push_ahead_label);
    let push_behind_icon = gtk::Image::from_icon_name("go-down-symbolic");
    let push_behind_label = gtk::Label::new(None);
    let push_behind_box = git_count_badge(&push_behind_icon, &push_behind_label);
    let push_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    push_content.append(&push_icon);
    push_content.append(&push_spinner);
    push_content.append(&push_label);
    push_content.append(&push_ahead_box);
    push_content.append(&push_behind_box);
    let push_button = gtk::Button::builder()
        .child(&push_content)
        .tooltip_text("Last fetched: unknown")
        .build();
    push_button.set_sensitive(false);

    let run_targets = Rc::new(RefCell::new(Vec::new()));
    let quick_actions = Rc::new(RefCell::new(Vec::new()));
    let quick_action_next_id = Rc::new(Cell::new(1));
    let quick_action_config_repo = Rc::new(RefCell::new(None));
    let quick_action_runner = Rc::new(RefCell::new(None));
    let quick_action_state_changed = Rc::new(RefCell::new(None));
    let quick_action_add_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add quick action")
        .build();
    quick_action_add_button.add_css_class("flat");
    let quick_actions_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .valign(gtk::Align::Center)
        .build();
    quick_actions_box.append(&quick_action_add_button);
    append_quick_action_button(
        &quick_actions_box,
        &quick_actions,
        &run_targets,
        &quick_action_next_id,
        &quick_action_runner,
        &quick_action_state_changed,
        None,
    );

    let terminal_toggle_button = gtk::ToggleButton::builder()
        .icon_name("utilities-terminal-symbolic")
        .tooltip_text("Show terminal")
        .build();
    terminal_toggle_button.add_css_class("flat");

    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(true)
        .show_start_title_buttons(true)
        .title_widget(&widgets::blank_title())
        .build();
    header.pack_start(&branch_picker.button);
    header.pack_end(&push_button);
    header.pack_end(&terminal_toggle_button);
    header.pack_end(&quick_actions_box);

    let main = MainContent::new();
    let terminal = terminal::TerminalPanel::new();
    let work_area = gtk::Paned::new(gtk::Orientation::Vertical);
    work_area.set_start_child(Some(&main.root));
    work_area.set_end_child(Some(&terminal.root));
    work_area.set_resize_start_child(true);
    work_area.set_shrink_start_child(true);
    work_area.set_resize_end_child(false);
    work_area.set_shrink_end_child(true);
    work_area.set_position(540);

    let root = adw::ToolbarView::new();
    root.add_top_bar(&header);
    root.set_content(Some(&work_area));

    let pane = ContentPane {
        root,
        toast_overlay: main.root,
        push_button,
        terminal_toggle_button,
        branch_picker,
        header,
        quick_actions_box,
        quick_action_add_button,
        quick_actions,
        quick_action_next_id,
        quick_action_config_repo,
        quick_action_runner,
        quick_action_state_changed,
        run_targets,
        run_targets_signature: RefCell::new(None),
        terminal,
        push_icon,
        push_spinner,
        push_label,
        push_ahead_box,
        push_behind_box,
        push_ahead_label,
        push_behind_label,
        git_action_progress: RefCell::new(None),
        page_slot: main.page_slot,
    };

    if let Some(snapshot) = snapshot {
        pane.update(snapshot, None, false, false);
    }

    pane
}

fn git_count_badge(icon: &gtk::Image, label: &gtk::Label) -> gtk::Box {
    icon.set_pixel_size(14);
    label.add_css_class("numeric");

    let badge = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(2)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    badge.append(icon);
    badge.append(label);
    badge
}

impl ContentPane {
    pub(super) fn page_slot(&self) -> gtk::Box {
        self.page_slot.clone()
    }

    pub(super) fn set_workspace_color_active(&self, active: bool) {
        if active {
            self.header.add_css_class("workspace-titlebar-color");
        } else {
            self.header.remove_css_class("workspace-titlebar-color");
        }
    }

    pub fn update(
        &self,
        snapshot: &RepositorySnapshot,
        message: Option<&str>,
        action_running: bool,
        show_toast: bool,
    ) {
        self.branch_picker.set_button_label(&snapshot.branch);
        self.update_branch_picker(snapshot);
        self.configure_git_action(snapshot, action_running);

        if show_toast && let Some(msg) = message {
            self.toast_overlay.add_toast(adw::Toast::new(msg));
        }
    }

    pub(super) fn show_toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    pub fn set_error(&self, message: &str) {
        self.clear_git_action_progress();
        self.branch_picker.set_button_label("Branch");
        self.push_icon.set_icon_name(Some("view-refresh-symbolic"));
        self.push_icon.set_visible(true);
        self.push_spinner.set_visible(false);
        self.push_label.set_label("Fetch remote");
        self.push_ahead_box.set_visible(false);
        self.push_behind_box.set_visible(false);
        self.push_button.set_sensitive(false);
        self.push_button.set_tooltip_text(Some(message));
    }

    pub fn set_git_action_progress(&self, progress: &str) {
        let progress = progress.trim();
        if progress.is_empty() {
            return;
        }
        if self.git_action_progress.borrow().as_deref() == Some(progress) {
            return;
        }

        self.git_action_progress.replace(Some(progress.to_string()));
        self.push_label.set_label(progress);
        self.push_button.set_tooltip_text(Some(progress));
    }

    pub fn clear_git_action_progress(&self) {
        self.git_action_progress.borrow_mut().take();
    }

    pub fn refresh_run_targets(&self, repo_path: &Path) {
        self.load_quick_action_config(repo_path);

        let signature = quick_action::targets_signature(repo_path);
        let previous_signature = self.run_targets_signature.borrow().clone();
        if previous_signature.as_ref() == Some(&signature) {
            return;
        }

        let targets = quick_action::discover(repo_path);
        log::debug!(
            "quick action target refresh for {} found {} target(s)",
            repo_path.display(),
            targets.len()
        );
        *self.run_targets_signature.borrow_mut() = Some(signature);
        if previous_signature.is_some()
            && self.run_targets.borrow().as_slice() == targets.as_slice()
        {
            return;
        }

        *self.run_targets.borrow_mut() = targets;

        for quick_action in self.quick_actions.borrow().iter() {
            quick_action.refresh();
        }
    }

    pub fn clear_run_targets(&self) {
        self.quick_action_config_repo.borrow_mut().take();
        if self.run_targets.borrow().is_empty() && self.run_targets_signature.borrow().is_none() {
            return;
        }
        log::debug!("quick action targets cleared for non-local workspace");
        self.run_targets.borrow_mut().clear();
        self.run_targets_signature.borrow_mut().take();
        for quick_action in self.quick_actions.borrow().iter() {
            quick_action.refresh();
        }
    }

    fn load_quick_action_config(&self, repo_path: &Path) {
        if self
            .quick_action_config_repo
            .borrow()
            .as_deref()
            .is_some_and(|path| path == repo_path)
        {
            return;
        }

        let configs = git::quick_action_config(repo_path).unwrap_or_else(|| {
            vec![QuickActionConfig {
                selected_target_id: None,
            }]
        });
        log::debug!(
            "loaded quick action config repo={} actions={}",
            repo_path.display(),
            configs.len()
        );

        {
            let mut quick_actions = self.quick_actions.borrow_mut();
            for quick_action in quick_actions.iter() {
                self.quick_actions_box.remove(&quick_action.button);
            }
            quick_actions.clear();
        }
        self.quick_action_next_id.set(1);

        for config in configs {
            append_quick_action_button(
                &self.quick_actions_box,
                &self.quick_actions,
                &self.run_targets,
                &self.quick_action_next_id,
                &self.quick_action_runner,
                &self.quick_action_state_changed,
                config.selected_target_id,
            );
        }
        *self.quick_action_config_repo.borrow_mut() = Some(repo_path.to_path_buf());
    }

    pub fn connect_repository_actions<C: RepositoryActionContext + 'static>(&self, context: C) {
        let suppress_terminal_auto_open = Rc::new(Cell::new(false));
        let application = context.window().application();

        self.terminal.connect_empty({
            let terminal_toggle_button = self.terminal_toggle_button.clone();
            let suppress_terminal_auto_open = suppress_terminal_auto_open.clone();
            let application = application.clone();

            move || {
                suppress_terminal_auto_open.set(true);
                terminal_toggle_button.set_active(false);
                suppress_terminal_auto_open.set(false);
                terminal_toggle_button.set_tooltip_text(Some("Show terminal"));
                if let Some(application) = application.as_ref() {
                    set_terminal_conflicting_accels_enabled(application, true);
                }
            }
        });

        if let Some(application) = application.as_ref() {
            let application = application.clone();
            self.terminal.connect_focus_changed(move |focused| {
                set_terminal_conflicting_accels_enabled(&application, !focused);
            });
        }

        self.terminal.connect_activation({
            let context = context.clone();

            move |activation| handle_terminal_activation(&context, activation)
        });

        self.terminal_toggle_button.connect_toggled({
            let context = context.clone();
            let terminal = self.terminal.clone();
            let suppress_terminal_auto_open = suppress_terminal_auto_open.clone();
            let application = application.clone();

            move |button| {
                let visible = button.is_active();
                terminal.set_visible(visible);
                button.set_tooltip_text(Some(if visible {
                    "Hide terminal"
                } else {
                    "Show terminal"
                }));
                if let Some(application) = application.as_ref() {
                    set_terminal_conflicting_accels_enabled(application, !visible);
                }

                if visible && !terminal.has_sessions() && !suppress_terminal_auto_open.get() {
                    match provider_shell_command(&context)
                        .and_then(|(command, title)| terminal.run_shell_command(&command, &title))
                    {
                        Ok(()) => context.refresh(Some("Opened in terminal.".to_string())),
                        Err(err) => show_error_dialog(&context.window(), "Open Failed", &err),
                    }
                }
            }
        });

        self.push_button.connect_clicked({
            let context = context.clone();
            let push_button = self.push_button.clone();

            move |_| {
                play_button_feedback(&push_button);
                context.run_git_action();
            }
        });

        self.terminal.add_button.connect_clicked({
            let context = context.clone();
            let terminal = self.terminal.clone();
            let terminal_toggle_button = self.terminal_toggle_button.clone();
            let suppress_terminal_auto_open = suppress_terminal_auto_open.clone();

            move |_| {
                open_embedded_terminal(
                    &context,
                    &terminal,
                    &terminal_toggle_button,
                    &suppress_terminal_auto_open,
                )
            }
        });

        *self.quick_action_runner.borrow_mut() = Some(Rc::new({
            let context = context.clone();
            let terminal = self.terminal.clone();
            let terminal_toggle_button = self.terminal_toggle_button.clone();
            let suppress_terminal_auto_open = suppress_terminal_auto_open.clone();

            move |target| {
                run_target(
                    &context,
                    &target,
                    &terminal,
                    &terminal_toggle_button,
                    &suppress_terminal_auto_open,
                );
            }
        }));
        *self.quick_action_state_changed.borrow_mut() = Some(Rc::new({
            let quick_actions = self.quick_actions.clone();
            let context = context.clone();

            move || save_quick_action_state(&context, &quick_actions)
        }));

        self.quick_action_add_button.connect_clicked({
            let context = context.clone();
            let quick_actions_box = self.quick_actions_box.downgrade();
            let quick_actions = self.quick_actions.clone();
            let run_targets = self.run_targets.clone();
            let quick_action_next_id = self.quick_action_next_id.clone();
            let quick_action_runner = self.quick_action_runner.clone();
            let quick_action_state_changed = self.quick_action_state_changed.clone();

            move |_| {
                let Some(quick_actions_box) = quick_actions_box.upgrade() else {
                    return;
                };
                append_quick_action_button(
                    &quick_actions_box,
                    &quick_actions,
                    &run_targets,
                    &quick_action_next_id,
                    &quick_action_runner,
                    &quick_action_state_changed,
                    None,
                );
                save_quick_action_state(&context, &quick_actions);
            }
        });
    }

    pub fn run_terminal_command(
        &self,
        command: &terminal::CommandSpec,
        title: &str,
    ) -> Result<(), String> {
        self.terminal.run(command, title)?;
        self.terminal_toggle_button.set_active(true);
        self.terminal_toggle_button
            .set_tooltip_text(Some("Hide terminal"));
        Ok(())
    }

    pub(crate) fn run_shell_command(
        &self,
        command: &crate::system::capabilities::shell::ShellCommandSpec,
        title: &str,
    ) -> Result<(), String> {
        self.terminal.run_shell_command(command, title)?;
        self.terminal_toggle_button.set_active(true);
        self.terminal_toggle_button
            .set_tooltip_text(Some("Hide terminal"));
        Ok(())
    }

    fn configure_git_action(&self, snapshot: &RepositorySnapshot, action_running: bool) {
        let action_label = git_action_label(snapshot);
        let progress_label = self.git_action_progress.borrow().clone();
        let label = if action_running {
            progress_label.as_deref().unwrap_or(&action_label)
        } else {
            action_label.as_str()
        };

        self.push_label.set_label(label);
        self.push_icon
            .set_icon_name(Some(git_action_icon_name(snapshot)));
        self.push_icon.set_visible(!action_running);
        self.push_spinner.set_visible(action_running);
        self.push_ahead_box
            .set_visible(!action_running && snapshot.ahead > 0);
        self.push_ahead_label.set_label(&snapshot.ahead.to_string());
        self.push_behind_box
            .set_visible(!action_running && snapshot.behind > 0);
        self.push_behind_label
            .set_label(&snapshot.behind.to_string());
        self.push_button.set_sensitive(
            !action_running && (snapshot.remote_name.is_some() || !snapshot.branch.is_empty()),
        );
        let tooltip_label = if action_running {
            progress_label.as_deref().unwrap_or(&action_label)
        } else {
            action_label.as_str()
        };
        self.push_button.set_tooltip_text(Some(&format!(
            "{}\n{}",
            tooltip_label,
            git_action_tooltip(snapshot, action_running)
        )));
    }

    fn update_branch_picker(&self, snapshot: &RepositorySnapshot) {
        self.branch_picker.set_tab(branches_tab(snapshot));
    }

    pub(super) fn set_pull_requests_loading(&self) {
        self.branch_picker.set_tab(pull_requests_tab(
            &[],
            TabbedPickerStatus::Loading("Loading pull requests...".to_string()),
        ));
    }

    pub(super) fn set_pull_requests(&self, pull_requests: Vec<PullRequestInfo>) {
        log::debug!(
            "branch picker pull requests updated count={}",
            pull_requests.len()
        );
        self.branch_picker.set_tab(pull_requests_tab(
            &pull_requests,
            if pull_requests.is_empty() {
                TabbedPickerStatus::Empty("No open pull requests.".to_string())
            } else {
                TabbedPickerStatus::Ready
            },
        ));
    }

    pub(super) fn set_pull_requests_error(&self, message: &str) {
        log::warn!("branch picker pull request load failed: {message}");
        self.branch_picker.set_tab(pull_requests_tab(
            &[],
            TabbedPickerStatus::Error(message.to_string()),
        ));
    }
}

fn handle_terminal_activation<C: RepositoryActionContext + 'static>(
    context: &C,
    activation: terminal::TerminalActivation,
) {
    match activation {
        terminal::TerminalActivation::Url(url) => confirm_open_terminal_url(context.clone(), url),
        terminal::TerminalActivation::File(file) => open_terminal_file_location(context, &file),
    }
}

fn confirm_open_terminal_url<C: RepositoryActionContext + 'static>(context: C, url: String) {
    let Some(desktop_opener) = context.desktop_opener() else {
        show_error_dialog(
            &context.window(),
            "Open Link Failed",
            "Opening links is unavailable for this workspace.",
        );
        log::warn!("terminal url activation failed reason=no-desktop-opener url={url}");
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

    let window = context.window();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response.as_str() != "open" {
            log::debug!("terminal url activation cancelled url={url}");
            return;
        }

        match desktop_opener.open_url(&url, DesktopOpenActivation::default()) {
            Ok(message) => {
                log::info!("terminal url opened url={url} message={message}");
                context.show_toast(&message);
            }
            Err(err) => {
                log::warn!("terminal url activation failed url={url}: {err}");
                show_error_dialog(&context.window(), "Open Link Failed", &err);
            }
        }
    });
}

#[derive(Clone, Debug)]
struct TerminalFileLocation {
    path: String,
    line: Option<usize>,
    column: Option<usize>,
}

fn open_terminal_file_location<C: RepositoryActionContext>(
    context: &C,
    file: &terminal::TerminalFileActivation,
) {
    let location = parse_terminal_file_location(&file.target);
    let Some(terminal_links) = context.terminal_links() else {
        let message = "Terminal link navigation is unavailable for this workspace.".to_string();
        log::warn!(
            "terminal file activation failed target={} launch_dir={} reason=no-terminal-link-capability",
            file.target,
            file.launch_dir
        );
        context.show_toast(&message);
        return;
    };
    let target = match terminal_links.resolve_file(&file.launch_dir, &location.path) {
        Ok(path) => path,
        Err(err) => {
            log::warn!(
                "terminal file activation failed target={} launch_dir={}: {}",
                file.target,
                file.launch_dir,
                err
            );
            context.show_toast(&err);
            return;
        }
    };

    let path = match target {
        TerminalLinkTarget::Workspace(path) => path,
        TerminalLinkTarget::External(path) => {
            open_external_terminal_file(context, file, &path);
            return;
        }
    };

    match context.dispatch_command(PageCommand::OpenFileLocation {
        path: path.relative_or_empty().to_string(),
        line: location.line,
        column: location.column,
    }) {
        PageCommandResult::Ignored => {
            let message = "Files are unavailable for this workspace.".to_string();
            log::warn!(
                "terminal file activation ignored resolved_path={} target={}",
                path.display(),
                file.target
            );
            context.show_toast(&message);
        }
        PageCommandResult::Handled | PageCommandResult::HandledAndActivate => {
            log::info!(
                "terminal file activation dispatched target={} resolved_path={} line={:?} column={:?}",
                file.target,
                path.display(),
                location.line,
                location.column
            );
        }
    }
}

fn open_external_terminal_file<C: RepositoryActionContext>(
    context: &C,
    file: &terminal::TerminalFileActivation,
    path: &WorkspacePath,
) {
    let message = "Opening files outside the workspace is unavailable.".to_string();
    log::warn!(
        "terminal external file activation failed target={} path={} reason=no-node-path",
        file.target,
        path.absolute
    );
    context.show_toast(&message);
}

fn parse_terminal_file_location(target: &str) -> TerminalFileLocation {
    let target = target
        .strip_prefix("file://")
        .unwrap_or(target)
        .trim()
        .to_string();
    let mut path = target.as_str();
    let mut line = None;
    let mut column = None;

    if let Some((before, last)) = path.rsplit_once(':')
        && let Ok(value) = last.parse::<usize>()
        && value > 0
    {
        if let Some((before_line, maybe_line)) = before.rsplit_once(':')
            && let Ok(line_value) = maybe_line.parse::<usize>()
            && line_value > 0
        {
            path = before_line;
            line = Some(line_value);
            column = Some(value);
        } else {
            path = before;
            line = Some(value);
        }
    }

    TerminalFileLocation {
        path: path.to_string(),
        line,
        column,
    }
}

fn set_terminal_conflicting_accels_enabled(app: &gtk::Application, enabled: bool) {
    for (action, accels) in TERMINAL_CONFLICTING_ACCELS {
        if enabled {
            app.set_accels_for_action(action, accels);
        } else {
            app.set_accels_for_action(action, &[]);
        }
    }
}

fn play_button_feedback(button: &gtk::Button) {
    button.set_state_flags(gtk::StateFlags::ACTIVE, false);

    let button = button.clone();
    gtk::glib::timeout_add_local_once(Duration::from_millis(160), move || {
        button.unset_state_flags(gtk::StateFlags::ACTIVE);
    });
}

fn empty_branch_tabs() -> Vec<TabbedPickerTab> {
    vec![
        TabbedPickerTab::new(
            "branches",
            "Branches",
            vec![TabbedPickerGroup::unlabelled(Vec::new())],
        )
        .status(TabbedPickerStatus::Empty("No branches found.".to_string())),
        pull_requests_tab(
            &[],
            TabbedPickerStatus::Empty("Open a repository with GitHub pull requests.".to_string()),
        ),
    ]
}

fn branch_picker_tabs(snapshot: &RepositorySnapshot) -> Vec<TabbedPickerTab> {
    vec![
        branches_tab(snapshot),
        pull_requests_tab(
            &[],
            TabbedPickerStatus::Empty("Open the picker to load pull requests.".to_string()),
        ),
    ]
}

fn branches_tab(snapshot: &RepositorySnapshot) -> TabbedPickerTab {
    let mut default = Vec::new();
    let mut recent = Vec::new();
    let mut other = Vec::new();

    for branch in snapshot.branches.iter() {
        if branch.is_default {
            default.push(branch_picker_item(branch));
        } else if branch.is_recent {
            recent.push(branch_picker_item(branch));
        } else if !branch.name.starts_with("github-desktop-") {
            other.push(branch_picker_item(branch));
        }
    }

    let mut groups = Vec::new();
    if !default.is_empty() {
        groups.push(TabbedPickerGroup::new("Default branch", default));
    }
    if !recent.is_empty() {
        groups.push(TabbedPickerGroup::new("Recent branches", recent));
    }
    groups.push(TabbedPickerGroup::unlabelled(other));

    log::debug!(
        "branch picker grouped branches branch={} groups={} total={}",
        snapshot.branch,
        groups.len(),
        snapshot.branches.len()
    );

    TabbedPickerTab::new("branches", "Branches", groups).status(if snapshot.branches.is_empty() {
        TabbedPickerStatus::Empty("No branches found.".to_string())
    } else {
        TabbedPickerStatus::Ready
    })
}

fn branch_picker_item(branch: &crate::git::BranchInfo) -> TabbedPickerItem {
    let icon = if branch.is_current {
        "object-select-symbolic"
    } else {
        "branch-symbolic"
    };
    let id = match branch.kind {
        crate::git::BranchKind::Local => format!("branch:{}", branch.name),
        crate::git::BranchKind::Remote => format!("remote:{}", branch.name),
    };
    let subtitle = match branch.kind {
        crate::git::BranchKind::Local => None,
        crate::git::BranchKind::Remote => Some("Remote branch".to_string()),
    };
    let mut item = TabbedPickerItem::new(id, branch.name.clone(), icon).selected(branch.is_current);
    if let Some(subtitle) = subtitle {
        item = item.subtitle(subtitle);
    }
    item
}

fn pull_requests_tab(
    pull_requests: &[PullRequestInfo],
    status: TabbedPickerStatus,
) -> TabbedPickerTab {
    let items = pull_requests
        .iter()
        .map(|pull_request| {
            let subtitle = format!(
                "#{} opened by {}{}",
                pull_request.number,
                pull_request.author,
                if pull_request.is_draft {
                    " · Draft"
                } else {
                    ""
                }
            );
            TabbedPickerItem::new(
                format!("pr:{}", pull_request.number),
                pull_request.title.clone(),
                "branch-fork-symbolic",
            )
            .subtitle(subtitle)
        })
        .collect::<Vec<_>>();

    TabbedPickerTab::new(
        "pull-requests",
        "Pull requests",
        vec![TabbedPickerGroup::unlabelled(items)],
    )
    .badge((!pull_requests.is_empty()).then_some(pull_requests.len()))
    .status(status)
}

struct QuickActionButton {
    id: u64,
    button: adw::SplitButton,
    run_content: gtk::Box,
    run_icon: gtk::Image,
    label: gtk::Label,
    list: gtk::ListBox,
    search_entry: gtk::SearchEntry,
    delete_button: gtk::Button,
    targets: Rc<RefCell<Vec<RunItem>>>,
    selected_target: Rc<RefCell<Option<RunItem>>>,
    saved_selected_target_id: Rc<RefCell<Option<String>>>,
    runner: Rc<RefCell<Option<Rc<dyn Fn(RunItem)>>>>,
    state_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    target_reconciler: Rc<RefCell<QuickActionTargetReconciler>>,
}

type QuickActionTargetReconciler =
    craic_diff_ui::gtk::ListBoxReconciler<String, QuickActionTargetRenderState>;

#[derive(Clone, PartialEq, Eq)]
enum QuickActionTargetRenderState {
    Target {
        label: String,
        icon_name: &'static str,
        selected: bool,
    },
    Empty {
        label: String,
    },
}

fn install_quick_action_css() {
    if QUICK_ACTION_CSS_INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }

    let Some(display) = gtk::gdk::Display::default() else {
        QUICK_ACTION_CSS_INSTALLED.store(false, Ordering::Relaxed);
        log::warn!("quick action CSS install skipped: no GTK display");
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        r#"
        popover.quick-action-popover contents {
            padding: 0;
        }
        "#,
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    log::debug!("quick action CSS installed");
}

impl QuickActionButton {
    fn new(
        id: u64,
        targets: Rc<RefCell<Vec<RunItem>>>,
        runner: Rc<RefCell<Option<Rc<dyn Fn(RunItem)>>>>,
        state_changed: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
        saved_selected_target_id: Option<String>,
    ) -> Rc<Self> {
        install_quick_action_css();

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search quick actions")
            .hexpand(true)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let search_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        search_row.append(&search_entry);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(96)
            .max_content_height(260)
            .propagate_natural_height(true)
            .child(&list)
            .build();

        let separator = gtk::Separator::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        let delete_button = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .tooltip_text("Delete quick action")
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::End)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        delete_button.add_css_class("flat");
        delete_button.add_css_class("circular");
        let delete_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        delete_row.append(&delete_button);

        let popover_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        popover_content.append(&search_row);
        popover_content.append(&scroller);
        popover_content.append(&separator);
        popover_content.append(&delete_row);

        let popover = gtk::Popover::builder()
            .width_request(320)
            .child(&popover_content)
            .build();
        popover.add_css_class("quick-action-popover");

        let run_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        run_icon.set_visible(false);
        let label = gtk::Label::builder()
            .label("(empty)")
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(18)
            .build();
        let button_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        button_content.append(&run_icon);
        button_content.append(&label);
        let button = adw::SplitButton::builder()
            .child(&button_content)
            .popover(&popover)
            .dropdown_tooltip("Choose quick action")
            .tooltip_text("Run selected quick action")
            .build();
        button.add_css_class("flat");

        let quick_action = Rc::new(Self {
            id,
            button,
            run_content: button_content,
            run_icon,
            label,
            list,
            search_entry,
            delete_button,
            targets,
            selected_target: Rc::new(RefCell::new(None)),
            saved_selected_target_id: Rc::new(RefCell::new(saved_selected_target_id)),
            runner,
            state_changed,
            target_reconciler: Rc::new(RefCell::new(QuickActionTargetReconciler::new())),
        });
        quick_action.refresh();
        quick_action.connect_search();
        quick_action.connect_target_activation();
        quick_action.connect_popover();
        quick_action.connect_run();
        quick_action
    }

    fn refresh(&self) {
        let saved_id = self.saved_selected_target_id.borrow().clone();
        let selected_id = self
            .selected_target
            .borrow()
            .as_ref()
            .map(|target| target.id.clone())
            .or_else(|| saved_id.clone());
        let targets = self.targets.borrow();
        let selected = match selected_id {
            Some(id) => targets.iter().find(|target| target.id == id).cloned(),
            None => targets.first().cloned(),
        };

        if let Some(selected) = selected {
            select_quick_action_target(
                &self.button,
                &self.run_content,
                &self.run_icon,
                &self.label,
                &self.selected_target,
                &self.saved_selected_target_id,
                selected,
            );
        } else {
            self.selected_target.borrow_mut().take();
            self.label.set_label("(empty)");
            self.run_icon.set_visible(false);
            self.run_content.set_sensitive(false);
            self.button.set_tooltip_text(Some(if targets.is_empty() {
                "No quick actions found"
            } else {
                "Saved quick action not found"
            }));
        }

        let selected_id = self
            .selected_target
            .borrow()
            .as_ref()
            .map(|target| target.id.clone());
        fill_quick_action_targets(
            &self.list,
            &self.target_reconciler,
            &targets,
            &self.search_entry.text(),
            selected_id.as_deref(),
        );
    }

    fn connect_search(&self) {
        self.search_entry.connect_search_changed({
            let list = self.list.clone();
            let targets = self.targets.clone();
            let selected_target = self.selected_target.clone();
            let target_reconciler = self.target_reconciler.clone();

            move |entry| {
                let selected_id = selected_target
                    .borrow()
                    .as_ref()
                    .map(|target| target.id.clone());
                fill_quick_action_targets(
                    &list,
                    &target_reconciler,
                    &targets.borrow(),
                    &entry.text(),
                    selected_id.as_deref(),
                );
            }
        });
    }

    fn connect_target_activation(&self) {
        self.list.connect_row_activated({
            let button = self.button.downgrade();
            let run_content = self.run_content.downgrade();
            let run_icon = self.run_icon.downgrade();
            let label = self.label.downgrade();
            let targets = self.targets.clone();
            let selected_target = self.selected_target.clone();
            let saved_selected_target_id = self.saved_selected_target_id.clone();
            let popover = self.button.popover().map(|popover| popover.downgrade());
            let state_changed = self.state_changed.clone();

            move |_, row| {
                let target_id = row.widget_name().to_string();
                if target_id.is_empty() {
                    return;
                }
                let (Some(button), Some(run_content), Some(run_icon), Some(label)) = (
                    button.upgrade(),
                    run_content.upgrade(),
                    run_icon.upgrade(),
                    label.upgrade(),
                ) else {
                    return;
                };

                let Some(target) = targets
                    .borrow()
                    .iter()
                    .find(|target| target.id == target_id)
                    .cloned()
                else {
                    return;
                };

                select_quick_action_target(
                    &button,
                    &run_content,
                    &run_icon,
                    &label,
                    &selected_target,
                    &saved_selected_target_id,
                    target,
                );
                if let Some(state_changed) = state_changed.borrow().as_ref() {
                    state_changed();
                }
                if let Some(popover) = popover.as_ref().and_then(|popover| popover.upgrade()) {
                    popover.popdown();
                }
            }
        });
    }

    fn connect_popover(&self) {
        if let Some(popover) = self.button.popover() {
            popover.connect_closed({
                let search_entry = self.search_entry.clone();
                move |_| {
                    search_entry.set_text("");
                }
            });

            popover.connect_show({
                let list = self.list.clone();
                let target_reconciler = self.target_reconciler.clone();
                let targets = self.targets.clone();
                let search_entry = self.search_entry.clone();
                let selected_target = self.selected_target.clone();
                move |_| {
                    let selected_id = selected_target
                        .borrow()
                        .as_ref()
                        .map(|target| target.id.clone());
                    fill_quick_action_targets(
                        &list,
                        &target_reconciler,
                        &targets.borrow(),
                        &search_entry.text(),
                        selected_id.as_deref(),
                    );
                }
            });
        }
    }

    fn connect_run(&self) {
        self.button.connect_clicked({
            let selected_target = self.selected_target.clone();
            let runner = self.runner.clone();

            move |_| {
                let target = selected_target.borrow().clone();
                let Some(target) = target else {
                    return;
                };
                if let Some(runner) = runner.borrow().as_ref() {
                    runner(target);
                }
            }
        });
    }

    fn selected_target_id(&self) -> Option<String> {
        self.selected_target
            .borrow()
            .as_ref()
            .map(|target| target.id.clone())
            .or_else(|| self.saved_selected_target_id.borrow().clone())
    }
}

fn append_quick_action_button(
    quick_actions_box: &gtk::Box,
    quick_actions: &Rc<RefCell<Vec<Rc<QuickActionButton>>>>,
    targets: &Rc<RefCell<Vec<RunItem>>>,
    next_id: &Rc<Cell<u64>>,
    runner: &Rc<RefCell<Option<Rc<dyn Fn(RunItem)>>>>,
    state_changed: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    selected_target_id: Option<String>,
) -> Rc<QuickActionButton> {
    let id = next_id.get();
    next_id.set(id.saturating_add(1));

    let quick_action = QuickActionButton::new(
        id,
        targets.clone(),
        runner.clone(),
        state_changed.clone(),
        selected_target_id,
    );
    quick_actions_box.append(&quick_action.button);
    connect_quick_action_delete(&quick_action, quick_actions_box, quick_actions);
    quick_actions.borrow_mut().push(quick_action.clone());
    quick_action
}

fn connect_quick_action_delete(
    quick_action: &Rc<QuickActionButton>,
    quick_actions_box: &gtk::Box,
    quick_actions: &Rc<RefCell<Vec<Rc<QuickActionButton>>>>,
) {
    let id = quick_action.id;
    let button = quick_action.button.downgrade();
    let quick_actions_box = quick_actions_box.downgrade();
    let quick_actions = Rc::downgrade(quick_actions);
    let state_changed = quick_action.state_changed.clone();

    quick_action.delete_button.connect_clicked(move |_| {
        if let Some(button) = button.upgrade() {
            if let Some(popover) = button.popover() {
                popover.popdown();
            }
            if let Some(quick_actions_box) = quick_actions_box.upgrade() {
                quick_actions_box.remove(&button);
            }
        }

        if let Some(quick_actions) = quick_actions.upgrade() {
            quick_actions
                .borrow_mut()
                .retain(|quick_action| quick_action.id != id);
        }
        if let Some(state_changed) = state_changed.borrow().as_ref() {
            state_changed();
        }
    });
}

fn select_quick_action_target(
    button: &adw::SplitButton,
    run_content: &gtk::Box,
    run_icon: &gtk::Image,
    label: &gtk::Label,
    selected_target: &Rc<RefCell<Option<RunItem>>>,
    saved_selected_target_id: &Rc<RefCell<Option<String>>>,
    target: RunItem,
) {
    label.set_label(&target.label);
    file_type::set_icon_for_name(run_icon, target.icon_name);
    run_icon.set_visible(true);
    run_content.set_sensitive(true);
    button.set_tooltip_text(Some(&format!("Run quick action: {}", target.label)));
    *saved_selected_target_id.borrow_mut() = Some(target.id.clone());
    *selected_target.borrow_mut() = Some(target);
}

fn quick_action_configs(
    quick_actions: &Rc<RefCell<Vec<Rc<QuickActionButton>>>>,
) -> Vec<QuickActionConfig> {
    quick_actions
        .borrow()
        .iter()
        .map(|quick_action| QuickActionConfig {
            selected_target_id: quick_action.selected_target_id(),
        })
        .collect()
}

fn save_quick_action_state<C: RepositoryActionContext>(
    context: &C,
    quick_actions: &Rc<RefCell<Vec<Rc<QuickActionButton>>>>,
) {
    let Some(repo_path) = context.local_workspace_path() else {
        log::debug!("quick action state save skipped for non-local workspace");
        return;
    };
    let configs = quick_action_configs(quick_actions);
    let action_count = configs.len();
    match git::save_quick_action_config(&repo_path, configs) {
        Ok(()) => log::info!(
            "saved quick action state repo={} actions={}",
            repo_path.display(),
            action_count
        ),
        Err(err) => log::warn!(
            "failed to save quick action state repo={} err={}",
            repo_path.display(),
            err
        ),
    }
}

fn fill_quick_action_targets(
    list: &gtk::ListBox,
    reconciler: &Rc<RefCell<QuickActionTargetReconciler>>,
    targets: &[RunItem],
    filter: &str,
    selected_id: Option<&str>,
) {
    let filter = filter.trim().to_lowercase();
    let mut elements = Vec::new();

    for target in targets {
        if !filter.is_empty() && !target.label.to_lowercase().contains(&filter) {
            continue;
        }

        let selected = selected_id.map_or(false, |id| id == target.id);
        elements.push(Element::new(
            target.id.clone(),
            QuickActionTargetRenderState::Target {
                label: target.label.clone(),
                icon_name: target.icon_name,
                selected,
            },
        ));
    }

    if elements.is_empty() {
        let label = if targets.is_empty() {
            "No quick actions found"
        } else {
            "No matching quick actions"
        };
        elements.push(Element::new(
            "__empty__".to_string(),
            QuickActionTargetRenderState::Empty {
                label: label.to_string(),
            },
        ));
    }

    reconciler.borrow_mut().reconcile(
        list,
        elements,
        PartialEqRenderState,
        |_, key, state| match state {
            QuickActionTargetRenderState::Target {
                label,
                icon_name,
                selected,
            } => quick_action_target_row(key, label, icon_name, *selected).upcast::<gtk::Widget>(),
            QuickActionTargetRenderState::Empty { label } => {
                quick_action_empty_row(label).upcast::<gtk::Widget>()
            }
        },
        |_, widget, _, next| update_quick_action_target_row(widget, next),
    );

    if let Some(selected_id) = selected_id {
        select_quick_action_row(list, selected_id);
    } else {
        list.unselect_all();
    }
}

fn quick_action_target_row(
    id: &str,
    text: &str,
    icon_name: &str,
    selected: bool,
) -> gtk::ListBoxRow {
    let icon = file_type::icon_for_name(icon_name);
    icon.set_pixel_size(16);
    icon.set_valign(gtk::Align::Center);
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    content.append(&icon);
    content.append(&label);

    let check = gtk::Image::builder()
        .icon_name("object-select-symbolic")
        .pixel_size(16)
        .valign(gtk::Align::Center)
        .visible(selected)
        .build();
    content.append(&check);

    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(id);
    row
}

fn quick_action_empty_row(label: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .xalign(0.0)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(8)
        .margin_end(8)
        .build();
    label.add_css_class("dim-label");

    let row = gtk::ListBoxRow::builder().child(&label).build();
    row.set_selectable(false);
    row.set_activatable(false);
    row
}

fn update_quick_action_target_row(widget: &gtk::Widget, state: &QuickActionTargetRenderState) {
    let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() else {
        return;
    };
    match state {
        QuickActionTargetRenderState::Target {
            label,
            icon_name,
            selected,
        } => {
            row.set_selectable(true);
            row.set_activatable(true);
            if let Some(content) = row.child().and_downcast::<gtk::Box>() {
                let mut child = content.first_child();
                let mut icon_widget = None;
                let mut label_widget = None;
                let mut check_widget = None;

                if let Some(w) = child {
                    icon_widget = w.clone().downcast::<gtk::Image>().ok();
                    child = w.next_sibling();
                    if let Some(ref lw) = child {
                        label_widget = lw.clone().downcast::<gtk::Label>().ok();
                        child = lw.next_sibling();
                        if let Some(ref cw) = child {
                            check_widget = cw.clone().downcast::<gtk::Image>().ok();
                        }
                    }
                }

                if let Some(iw) = icon_widget {
                    file_type::set_icon_for_name(&iw, icon_name);
                }
                if let Some(lw) = label_widget {
                    lw.set_label(label);
                }
                if let Some(cw) = check_widget {
                    cw.set_visible(*selected);
                }
            }
        }
        QuickActionTargetRenderState::Empty { label } => {
            row.set_selectable(false);
            row.set_activatable(false);
            if let Some(label_widget) = row.child().and_downcast::<gtk::Label>() {
                label_widget.set_label(label);
            }
        }
    }
}

fn select_quick_action_row(list: &gtk::ListBox, selected_id: &str) {
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if row.widget_name() == selected_id {
                list.select_row(Some(&row));
                return;
            }
        }
        child = next;
    }

    list.unselect_all();
}

fn run_target<C: RepositoryActionContext>(
    context: &C,
    target: &RunItem,
    terminal: &terminal::TerminalPanel,
    terminal_toggle_button: &gtk::ToggleButton,
    suppress_terminal_auto_open: &Rc<Cell<bool>>,
) {
    let Some(repo_path) = context.local_workspace_path() else {
        show_error_dialog(
            &context.window(),
            "Run Failed",
            "Quick actions are unavailable for remote workspaces.",
        );
        return;
    };
    let command = quick_action::command_for(&repo_path, target);

    show_terminal(
        terminal,
        terminal_toggle_button,
        suppress_terminal_auto_open,
    );
    match terminal.run(&command, &target.label) {
        Ok(()) => context.refresh(Some(format!("Started {} in terminal.", target.label))),
        Err(err) => show_error_dialog(&context.window(), "Run Failed", &err),
    }
}

fn open_embedded_terminal<C: RepositoryActionContext>(
    context: &C,
    terminal: &terminal::TerminalPanel,
    terminal_toggle_button: &gtk::ToggleButton,
    suppress_terminal_auto_open: &Rc<Cell<bool>>,
) {
    show_terminal(
        terminal,
        terminal_toggle_button,
        suppress_terminal_auto_open,
    );
    match provider_shell_command(context)
        .and_then(|(command, title)| terminal.run_shell_command(&command, &title))
    {
        Ok(()) => context.refresh(Some("Opened embedded terminal.".to_string())),
        Err(err) => show_error_dialog(&context.window(), "Open Failed", &err),
    }
}

fn provider_shell_command<C: RepositoryActionContext>(
    context: &C,
) -> Result<(crate::system::capabilities::shell::ShellCommandSpec, String), String> {
    let Some(shell) = context.shell() else {
        return Err("Terminal is unavailable for this workspace.".to_string());
    };
    let command = shell.interactive_shell(Some(&context.workspace_root()))?;
    let title = shell.command_display(&command);
    Ok((command, title))
}

fn show_terminal(
    terminal: &terminal::TerminalPanel,
    terminal_toggle_button: &gtk::ToggleButton,
    suppress_terminal_auto_open: &Rc<Cell<bool>>,
) {
    suppress_terminal_auto_open.set(true);
    terminal_toggle_button.set_active(true);
    suppress_terminal_auto_open.set(false);
    terminal.set_visible(true);
}

fn show_error_dialog(window: &adw::ApplicationWindow, heading: &str, message: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(message));
    dialog.add_response("ok", "OK");
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(Some(window));
}

struct MainContent {
    root: adw::ToastOverlay,
    page_slot: gtk::Box,
}

impl MainContent {
    fn new() -> Self {
        let toast_overlay = adw::ToastOverlay::new();
        let page_slot = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        toast_overlay.set_child(Some(&page_slot));

        Self {
            root: toast_overlay,
            page_slot,
        }
    }
}

pub(in crate::ui) fn centered_page(content: gtk::Box) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder()
        .maximum_size(640)
        .tightening_threshold(520)
        .child(&content)
        .build();

    let wrapper = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(32)
        .margin_end(32)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Start)
        .hexpand(true)
        .build();
    wrapper.append(&clamp);

    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&wrapper)
        .build()
}
fn git_action_label(snapshot: &RepositorySnapshot) -> String {
    let remote = snapshot.remote_name.as_deref().unwrap_or("remote");

    if !snapshot.has_upstream {
        if snapshot.remote_name.is_some() {
            "Publish branch".to_string()
        } else {
            "Publish repository".to_string()
        }
    } else {
        if snapshot.behind > 0 {
            format!("Pull {remote}")
        } else if snapshot.ahead > 0 {
            format!("Push {remote}")
        } else {
            format!("Fetch {remote}")
        }
    }
}

fn git_action_icon_name(snapshot: &RepositorySnapshot) -> &'static str {
    if !snapshot.has_upstream {
        "document-send-symbolic"
    } else if snapshot.behind > 0 {
        "go-down-symbolic"
    } else if snapshot.ahead > 0 {
        "view-refresh-symbolic"
    } else {
        "view-refresh-symbolic"
    }
}

fn git_action_tooltip(snapshot: &RepositorySnapshot, action_running: bool) -> String {
    let remote = snapshot.remote_name.as_deref().unwrap_or("remote");
    let action = if action_running {
        if !snapshot.has_upstream && snapshot.remote_name.is_none() {
            "Publishing repository to GitHub".to_string()
        } else if !snapshot.has_upstream {
            format!("Publishing to {remote}")
        } else if snapshot.behind > 0 {
            if snapshot.ahead > 0 {
                format!("Pulling from and pushing to {remote}")
            } else {
                format!("Pulling from {remote}")
            }
        } else if snapshot.ahead > 0 {
            format!("Pushing to {remote}")
        } else {
            format!("Fetching {remote}")
        }
    } else if snapshot.remote_name.is_some() {
        if !snapshot.has_upstream {
            format!("Publish {} to {remote}", snapshot.branch)
        } else if snapshot.behind > 0 {
            if snapshot.ahead > 0 {
                format!(
                    "Pull {} and push {} commits to/from {remote}",
                    snapshot.behind, snapshot.ahead
                )
            } else {
                format!("Pull {} commits from {remote}", snapshot.behind)
            }
        } else if snapshot.ahead > 0 {
            if snapshot.ahead == 1 {
                format!("Push 1 commit to {remote}")
            } else {
                format!("Push {} commits to {remote}", snapshot.ahead)
            }
        } else {
            format!("Fetch {remote}")
        }
    } else if !snapshot.branch.is_empty() {
        "Publish repository to GitHub".to_string()
    } else {
        "Repository is not initialized".to_string()
    };

    format!("{action}\n{}", last_fetch_text(snapshot.last_fetch_at))
}

fn last_fetch_text(last_fetch_at: Option<SystemTime>) -> String {
    let Some(last_fetch_at) = last_fetch_at else {
        return "Never fetched.".to_string();
    };
    let elapsed = SystemTime::now()
        .duration_since(last_fetch_at)
        .unwrap_or(Duration::ZERO);

    format!("Last fetched {}.", relative_time(elapsed))
}

fn relative_time(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..=4 => "just now".to_string(),
        5..=59 => plural(elapsed.as_secs(), "second"),
        60..=3_599 => plural(elapsed.as_secs() / 60, "minute"),
        3_600..=86_399 => plural(elapsed.as_secs() / 3_600, "hour"),
        86_400..=2_592_000 => plural(elapsed.as_secs() / 86_400, "day"),
        _ => plural(elapsed.as_secs() / 2_592_000, "month"),
    }
}

fn plural(value: u64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}

pub(in crate::ui) fn page(
    title: &gtk::Label,
    subtitle: &gtk::Label,
    body: &impl IsA<gtk::Widget>,
) -> gtk::Box {
    let heading = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    heading.append(title);
    heading.append(subtitle);

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(22)
        .build();
    page.append(&heading);
    page.append(body);
    page
}

pub(in crate::ui) struct SuggestionsPanel {
    pub(in crate::ui) root: gtk::Box,
    pub actions: SuggestionsActions,
}

pub struct SuggestionsActions {
    pub open_editor: gtk::Button,
    pub open_terminal: gtk::Button,
    pub show_files: gtk::Button,
    pub view_github: gtk::Button,
    pub git_button: gtk::Button,
    pub git_card: gtk::Box,
    pub git_title: gtk::Label,
    pub git_subtitle: gtk::Label,
}

impl SuggestionsPanel {
    pub(in crate::ui) fn new() -> Self {
        let open_editor_button = gtk::Button::with_label("Open");
        let open_terminal_button = gtk::Button::with_label("Open");
        let show_files_button = gtk::Button::with_label("Show");
        let view_github_button = gtk::Button::with_label("View");

        let git_title = widgets::heading("");
        let git_subtitle = widgets::muted("");
        let git_button = gtk::Button::builder()
            .valign(gtk::Align::Center)
            .halign(gtk::Align::End)
            .build();
        git_button.add_css_class("suggested-action");

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();
        text.append(&git_title);
        text.append(&git_subtitle);

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(20)
            .margin_end(20)
            .hexpand(true)
            .build();
        row.append(&text);
        row.append(&git_button);

        let git_card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .visible(false)
            .build();
        git_card.add_css_class("card");
        git_card.add_css_class("git-action-card");
        git_card.append(&row);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();

        root.append(&git_card);
        root.append(&action_card(
            "Open in editor",
            "Jump into the project files.",
            &open_editor_button,
        ));
        root.append(&action_card(
            "Open in Ghostty",
            "Open the repository in an external Ghostty window.",
            &open_terminal_button,
        ));

        root.append(&action_card(
            "Open in Files",
            "Open the repository folder in the external file manager.",
            &show_files_button,
        ));
        root.append(&action_card(
            "View on GitHub",
            "Open the remote repository.",
            &view_github_button,
        ));

        Self {
            root,
            actions: SuggestionsActions {
                open_editor: open_editor_button,
                open_terminal: open_terminal_button,
                show_files: show_files_button,
                view_github: view_github_button,
                git_button,
                git_card,
                git_title,
                git_subtitle,
            },
        }
    }
}

fn action_card(title: &str, subtitle: &str, button: &gtk::Button) -> gtk::Box {
    let title = clipped_label(&widgets::heading(title));
    let subtitle = clipped_label(&widgets::muted(subtitle));
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    text.append(&title);
    text.append(&subtitle);

    button.set_valign(gtk::Align::Center);
    button.set_halign(gtk::Align::End);

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(20)
        .margin_end(20)
        .hexpand(true)
        .build();
    row.append(&text);
    row.append(button);

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    card.add_css_class("card");
    card.append(&row);
    card
}

fn clipped_label(label: &gtk::Label) -> gtk::Label {
    label.set_wrap(false);
    label.set_ellipsize(pango::EllipsizeMode::End);
    label.clone()
}
