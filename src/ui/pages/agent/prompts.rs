use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

const PROMPT_REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);
const PROMPT_MONITOR_RATE_LIMIT_MS: i32 = 250;
const PROMPT_BUTTON_MIN_LABEL_CHARS: usize = 8;
const PROMPT_BUTTON_MAX_LABEL_CHARS: usize = 24;
const PROMPT_BUTTON_SIDE_MARGIN: i32 = 4;

type PromptSelectedCallback = Rc<dyn Fn(Result<PromptSelection, String>)>;

#[derive(Clone)]
pub(super) struct PromptSelection {
    pub(super) content: String,
}

pub(super) struct PromptBar {
    pub(super) root: gtk::Box,
    state: Rc<PromptBarState>,
}

struct PromptBarState {
    root: gtk::Box,
    buttons: gtk::Box,
    repo_path: RefCell<Option<PathBuf>>,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    debounce_source: RefCell<Option<glib::SourceId>>,
    prompt_files: RefCell<Vec<PromptFile>>,
    selected_callback: RefCell<Option<PromptSelectedCallback>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PromptFile {
    label: String,
    path: PathBuf,
}

impl PromptBar {
    pub(super) fn new() -> Self {
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(PROMPT_BUTTON_SIDE_MARGIN)
            .margin_end(PROMPT_BUTTON_SIDE_MARGIN)
            .build();

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .hexpand(true)
            .child(&buttons)
            .build();

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .visible(false)
            .build();
        root.append(&scroller);
        root.append(&separator);

        let state = Rc::new(PromptBarState {
            root: root.clone(),
            buttons,
            repo_path: RefCell::new(None),
            monitors: RefCell::new(Vec::new()),
            debounce_source: RefCell::new(None),
            prompt_files: RefCell::new(Vec::new()),
            selected_callback: RefCell::new(None),
        });

        Self { root, state }
    }

    pub(super) fn set_local_repo_path(&self, repo_path: Option<&Path>) {
        let repo_path = repo_path.map(Path::to_path_buf);
        if *self.state.repo_path.borrow() == repo_path {
            return;
        }

        stop_prompt_monitoring(&self.state);
        self.state.repo_path.replace(repo_path);
        log::debug!("Refreshing agent prompts after workspace path change");
        restart_prompt_monitors(&self.state);
        sync_prompt_buttons(&self.state);
    }

    pub(super) fn connect_prompt_selected<F>(&self, callback: F)
    where
        F: Fn(Result<PromptSelection, String>) + 'static,
    {
        self.state
            .selected_callback
            .replace(Some(Rc::new(callback)));
    }
}

impl Drop for PromptBar {
    fn drop(&mut self) {
        stop_prompt_monitoring(&self.state);
    }
}

fn restart_prompt_monitors(state: &Rc<PromptBarState>) {
    for monitor in state.monitors.borrow_mut().drain(..) {
        monitor.cancel();
    }

    let prompt_dirs = resolved_prompt_dirs(state);
    if prompt_dirs.is_empty() {
        log::debug!("Skipping agent prompt monitor because no prompt directories resolved");
        return;
    }

    let mut monitors = Vec::new();
    for prompts_dir in prompt_dirs {
        if prompts_dir.is_dir() {
            if let Some(monitor) = create_prompt_monitor(state, &prompts_dir) {
                monitors.push(monitor);
            }
        } else {
            log::debug!(
                "Skipping agent prompt monitor because {} is not a directory",
                prompts_dir.display()
            );
        }
    }
    state.monitors.replace(monitors);
}

fn stop_prompt_monitoring(state: &Rc<PromptBarState>) {
    if let Some(source_id) = state.debounce_source.borrow_mut().take() {
        source_id.remove();
    }

    for monitor in state.monitors.borrow_mut().drain(..) {
        monitor.cancel();
    }
}

fn create_prompt_monitor(state: &Rc<PromptBarState>, path: &Path) -> Option<gio::FileMonitor> {
    let file = gio::File::for_path(path);
    let flags = gio::FileMonitorFlags::WATCH_MOVES | gio::FileMonitorFlags::SEND_MOVED;
    let monitor = match file.monitor_directory(flags, None::<&gio::Cancellable>) {
        Ok(monitor) => monitor,
        Err(err) => {
            log::debug!(
                "Failed to monitor agent prompts at {}: {err}",
                path.display()
            );
            return None;
        }
    };
    monitor.set_rate_limit(PROMPT_MONITOR_RATE_LIMIT_MS);
    log::debug!("Monitoring agent prompts at {}", path.display());

    let state_weak = Rc::downgrade(state);
    monitor.connect_changed(move |_, _, _, event_type| {
        let Some(state) = state_weak.upgrade() else {
            return;
        };
        if prompt_monitor_event_should_sync(event_type) {
            log::debug!("Agent prompt directory changed; scheduling prompt refresh");
            queue_prompt_sync(&state);
        }
    });

    Some(monitor)
}

fn prompt_monitor_event_should_sync(event_type: gio::FileMonitorEvent) -> bool {
    !matches!(
        event_type,
        gio::FileMonitorEvent::PreUnmount | gio::FileMonitorEvent::Unmounted
    )
}

fn queue_prompt_sync(state: &Rc<PromptBarState>) {
    if state.debounce_source.borrow().is_some() {
        return;
    }

    let state_for_source = state.clone();
    let source_id = glib::timeout_add_local(PROMPT_REFRESH_DEBOUNCE, move || {
        state_for_source.debounce_source.borrow_mut().take();
        restart_prompt_monitors(&state_for_source);
        sync_prompt_buttons(&state_for_source);
        glib::ControlFlow::Break
    });
    state.debounce_source.replace(Some(source_id));
}

fn sync_prompt_buttons(state: &Rc<PromptBarState>) {
    let prompt_files = read_prompt_files(state);

    state.root.set_visible(!prompt_files.is_empty());
    if *state.prompt_files.borrow() == prompt_files {
        return;
    }

    clear_buttons(&state.buttons);
    state.prompt_files.replace(prompt_files.clone());

    let state_weak = Rc::downgrade(state);
    for prompt_file in prompt_files {
        add_prompt_button(&state_weak, &state.buttons, prompt_file);
    }
}

fn clear_buttons(buttons: &gtk::Box) {
    while let Some(child) = buttons.first_child() {
        buttons.remove(&child);
    }
}

fn add_prompt_button(
    state_weak: &Weak<PromptBarState>,
    buttons: &gtk::Box,
    prompt_file: PromptFile,
) {
    let icon = gtk::Image::from_icon_name("clipboard-symbolic");
    icon.set_pixel_size(16);

    let label_width_chars = prompt_file
        .label
        .chars()
        .count()
        .clamp(PROMPT_BUTTON_MIN_LABEL_CHARS, PROMPT_BUTTON_MAX_LABEL_CHARS)
        as i32;
    let label = gtk::Label::builder()
        .label(&prompt_file.label)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .width_chars(label_width_chars)
        .max_width_chars(PROMPT_BUTTON_MAX_LABEL_CHARS as i32)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    content.append(&icon);
    content.append(&label);

    let button = gtk::Button::builder().child(&content).build();
    if let Some(file_name) = prompt_file.path.file_name().and_then(|name| name.to_str()) {
        button.set_tooltip_text(Some(&format!("Send {file_name}")));
    }

    button.connect_clicked({
        let state_weak = state_weak.clone();
        let label = prompt_file.label;
        let path = prompt_file.path;

        move |_| {
            let Some(state) = state_weak.upgrade() else {
                return;
            };

            let result = read_prompt_selection(&label, &path);
            let callback = state.selected_callback.borrow().clone();
            if let Some(callback) = callback {
                callback(result);
            }
        }
    });

    buttons.append(&button);
}

fn read_prompt_files(state: &PromptBarState) -> Vec<PromptFile> {
    let prompt_dirs = resolved_prompt_dirs(state);
    if prompt_dirs.is_empty() {
        log::debug!("Skipping agent prompt scan because no prompt directories resolved");
        return Vec::new();
    }

    let mut prompt_files = Vec::new();
    for prompts_dir in prompt_dirs {
        let entries = match fs::read_dir(&prompts_dir) {
            Ok(entries) => entries,
            Err(err) => {
                log_prompt_dir_read_error(&prompts_dir, &err);
                continue;
            }
        };

        let before_len = prompt_files.len();
        prompt_files.extend(entries.flatten().filter_map(prompt_file_from_entry));
        log::debug!(
            "Loaded {} agent prompt file(s) from {}",
            prompt_files.len() - before_len,
            prompts_dir.display()
        );
    }

    prompt_files.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    prompt_files
}

fn resolved_prompt_dirs(state: &PromptBarState) -> Vec<PathBuf> {
    let repo_path = state.repo_path.borrow().clone();
    let prompt_dirs = crate::config::prompt_dirs(repo_path.as_deref());
    log::debug!(
        "Resolved {} agent prompt directories: {}",
        prompt_dirs.len(),
        prompt_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    prompt_dirs
}

fn log_prompt_dir_read_error(path: &Path, err: &io::Error) {
    if err.kind() == io::ErrorKind::NotFound {
        log::debug!(
            "Agent prompt directory {} does not exist; hiding prompt buttons",
            path.display()
        );
    } else {
        log::warn!(
            "Failed to list agent prompt directory {}: {err}",
            path.display()
        );
    }
}

fn prompt_file_from_entry(entry: fs::DirEntry) -> Option<PromptFile> {
    let path = entry.path();
    if !path.is_file() {
        return None;
    }

    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if !is_markdown {
        return None;
    }

    let label = path.file_stem()?.to_string_lossy().trim().to_string();
    if label.is_empty() {
        return None;
    }

    Some(PromptFile { label, path })
}

fn read_prompt_selection(label: &str, path: &Path) -> Result<PromptSelection, String> {
    fs::read_to_string(path)
        .map(|content| PromptSelection { content })
        .map_err(|err| {
            let message = format!(
                "Failed to read prompt {label} from {}: {err}",
                path.display()
            );
            log::warn!("{message}");
            message
        })
}
