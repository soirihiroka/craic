use crate::config;
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::ui::{
    canvas_scroll,
    components::{
        context_menu,
        search::{SearchOption, SearchPanel},
        terminal as terminal_component,
    },
};
use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::rc::Rc;
use vte4::prelude::*;

pub(crate) use crate::ui::components::terminal::{TerminalActivation, TerminalFileActivation};

const DEFAULT_COLUMNS: i64 = 100;
const DEFAULT_ROWS: i64 = 34;
const SESSION_RAIL_WIDTH: i32 = 120;
const SESSION_RAIL_PANEL_WIDTH: i32 = 132;
const TERMINAL_AUTO_CLOSE_IDLE_SECONDS: u64 = 60;
const TERM_NAME: &str = "xterm-256color";
const COLORTERM_NAME: &str = "truecolor";
const VTE_VERSION: &str = "8400";
const CTRL_BACKSPACE_SEQUENCE: &[u8] = b"\x17";
type EmptyHandlers = Rc<RefCell<Vec<Box<dyn Fn()>>>>;
type FocusHandlers = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;
type ActivationHandlers = Rc<RefCell<Vec<Box<dyn Fn(TerminalActivation)>>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    working_dir: PathBuf,
}

#[derive(Clone)]
pub struct TerminalPanel {
    pub root: gtk::Box,
    pub add_button: gtk::Button,
    search_panel: SearchPanel,
    search_options: TerminalSearchOptions,
    session_stack: gtk::Stack,
    session_rail: gtk::ListBox,
    sessions: Rc<RefCell<Vec<TerminalSession>>>,
    next_session_id: Rc<Cell<u64>>,
    empty_handlers: EmptyHandlers,
    focus_handlers: FocusHandlers,
    activation_handlers: ActivationHandlers,
}

#[derive(Clone)]
struct TerminalSession {
    id: u64,
    root: gtk::Overlay,
    row: gtk::ListBoxRow,
    terminal: vte4::Terminal,
    child_pid: Rc<Cell<Option<glib::Pid>>>,
    launch_kind: TerminalLaunchKind,
    state: Rc<Cell<TerminalSessionState>>,
    exit_success: Rc<Cell<bool>>,
    auto_close_source: Rc<RefCell<Option<glib::SourceId>>>,
}

#[derive(Clone)]
struct TerminalSearchOptions {
    case_sensitive: Rc<Cell<bool>>,
    whole_word: Rc<Cell<bool>>,
    regex: Rc<Cell<bool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalLaunchKind {
    Command,
    Shell,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalSessionState {
    Starting,
    Running,
    Exited,
    Closing,
}

#[derive(Clone, Copy, Debug)]
enum TerminalSearchMove {
    Keep,
    Previous,
    Next,
}

impl TerminalSearchOptions {
    fn new() -> Self {
        Self {
            case_sensitive: Rc::new(Cell::new(false)),
            whole_word: Rc::new(Cell::new(false)),
            regex: Rc::new(Cell::new(false)),
        }
    }
}

impl CommandSpec {
    fn display(&self) -> String {
        self.argv()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn argv(&self) -> Vec<OsString> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

impl TerminalPanel {
    pub fn new() -> Self {
        let add_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New terminal")
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .build();
        add_button.add_css_class("flat");
        add_button.add_css_class("circular");

        let add_button_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Fill)
            .vexpand(false)
            .margin_top(6)
            .margin_start(6)
            .margin_bottom(4)
            .build();
        add_button_row.append(&add_button);

        let session_stack = gtk::Stack::builder()
            .height_request(340)
            .hexpand(true)
            .vexpand(true)
            .build();

        let session_rail = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .hexpand(false)
            .build();
        session_rail.add_css_class("navigation-sidebar");

        let session_rail_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .width_request(SESSION_RAIL_WIDTH)
            .hexpand(false)
            .vexpand(false)
            .child(&session_rail)
            .build();

        let session_rail_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(SESSION_RAIL_PANEL_WIDTH)
            .hexpand(false)
            .vexpand(true)
            .build();
        session_rail_panel.append(&add_button_row);
        session_rail_panel.append(&session_rail_scroller);

        let terminal_area = libpanel::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .build();
        terminal_area.append(&session_stack);
        terminal_area.append(&session_rail_panel);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .height_request(340)
            .visible(false)
            .build();
        let search_panel = SearchPanel::new("Search Terminal");
        search_panel.set_clear_on_close(false);
        search_panel.set_options_visible(true);
        search_panel.set_navigation_visible(true);
        let search_widget = search_panel.widget();
        search_panel.set_key_capture_widget(&search_widget);
        search_panel.install_shortcuts(&search_widget);

        root.append(&separator);
        root.append(&search_widget);
        root.append(&terminal_area);

        let panel = Self {
            root,
            add_button,
            search_panel,
            search_options: TerminalSearchOptions::new(),
            session_stack,
            session_rail,
            sessions: Rc::new(RefCell::new(Vec::new())),
            next_session_id: Rc::new(Cell::new(1)),
            empty_handlers: Rc::new(RefCell::new(Vec::new())),
            focus_handlers: Rc::new(RefCell::new(Vec::new())),
            activation_handlers: Rc::new(RefCell::new(Vec::new())),
        };
        panel.connect_controls();
        panel.connect_search_controls();
        panel
    }

    pub fn set_visible(&self, visible: bool) {
        self.root.set_visible(visible);
        if visible {
            self.focus_active_terminal();
        }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.borrow().is_empty()
    }

    pub fn active_task_count(&self) -> usize {
        self.sessions
            .borrow()
            .iter()
            .filter(|session| active_task_name(session).is_some())
            .count()
    }

    pub fn connect_empty<F: Fn() + 'static>(&self, callback: F) {
        self.empty_handlers.borrow_mut().push(Box::new(callback));
    }

    pub fn connect_focus_changed<F: Fn(bool) + 'static>(&self, callback: F) {
        self.focus_handlers.borrow_mut().push(Box::new(callback));
    }

    pub(crate) fn connect_activation<F: Fn(TerminalActivation) + 'static>(&self, callback: F) {
        self.activation_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }

    pub fn run(&self, command: &CommandSpec, title: &str) -> Result<(), String> {
        self.set_visible(true);
        self.create_session(
            title,
            command,
            TerminalLaunchKind::Command,
            command.working_dir.to_string_lossy().as_ref(),
        )?;
        Ok(())
    }

    pub(crate) fn run_shell_command(
        &self,
        command: &ShellCommandSpec,
        title: &str,
    ) -> Result<(), String> {
        let launch_dir = command.working_dir.absolute.clone();
        let command = CommandSpec {
            program: command.program.clone(),
            args: command.args.clone(),
            working_dir: local_spawn_dir_for_shell_command(command),
        };
        self.set_visible(true);
        self.create_session(title, &command, TerminalLaunchKind::Shell, &launch_dir)?;
        Ok(())
    }

    fn connect_controls(&self) {
        self.session_rail.connect_row_selected({
            let sessions = self.sessions.clone();
            let session_stack = self.session_stack.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move |_, row| {
                if let Some(session) = row.and_then(|row| session_by_row(&sessions, row)) {
                    session_stack.set_visible_child(&session.root);
                    apply_terminal_search(
                        &session.terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Keep,
                    );
                    session.terminal.grab_focus();
                }
            }
        });
    }

    fn connect_search_controls(&self) {
        self.search_panel.connect_query_changed({
            let sessions = self.sessions.clone();
            let session_rail = self.session_rail.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move |_| {
                if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
        self.search_panel.connect_opened({
            let sessions = self.sessions.clone();
            let session_rail = self.session_rail.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
        self.search_panel.connect_closed({
            let sessions = self.sessions.clone();
            let session_rail = self.session_rail.clone();
            let search_panel = self.search_panel.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                    terminal.search_set_regex(None, 0);
                    search_panel.set_status("");
                    log::debug!("terminal search cleared on close");
                }
            }
        });
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::CaseSensitive,
            self.search_options.case_sensitive.clone(),
            self.sessions.clone(),
            self.session_rail.clone(),
            self.search_options.clone(),
        );
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::WholeWord,
            self.search_options.whole_word.clone(),
            self.sessions.clone(),
            self.session_rail.clone(),
            self.search_options.clone(),
        );
        connect_terminal_search_option(
            &self.search_panel,
            SearchOption::Regex,
            self.search_options.regex.clone(),
            self.sessions.clone(),
            self.session_rail.clone(),
            self.search_options.clone(),
        );
        self.search_panel.connect_previous({
            let sessions = self.sessions.clone();
            let session_rail = self.session_rail.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Previous,
                    );
                }
            }
        });
        self.search_panel.connect_next({
            let sessions = self.sessions.clone();
            let session_rail = self.session_rail.clone();
            let search_panel = self.search_panel.clone();
            let search_options = self.search_options.clone();

            move || {
                if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                    apply_terminal_search(
                        &terminal,
                        &search_panel,
                        &search_options,
                        TerminalSearchMove::Next,
                    );
                }
            }
        });
    }

    fn create_session(
        &self,
        title: &str,
        command: &CommandSpec,
        launch_kind: TerminalLaunchKind,
        launch_dir: &str,
    ) -> Result<TerminalSession, String> {
        let session_id = self.next_session_id.get();
        self.next_session_id.set(session_id + 1);

        let terminal = configured_terminal(config::load().font_sizes.shell);
        install_terminal_shortcuts(&terminal, &self.sessions, &self.search_panel);
        install_focus_tracking(&terminal, &self.focus_handlers);
        install_terminal_activation(&terminal, &self.activation_handlers, launch_dir.to_string());
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&terminal)
            .build();
        let autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&scroller));
        root.add_overlay(&autoscroll_marker);
        let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
        canvas_scroll::install_scrolled_window_middle_autoscroll_with_state(
            &scroller,
            &autoscroll_marker,
            &autoscroll,
            canvas_scroll::AutoscrollAxes::Vertical,
            "terminal",
            {
                let scroller = scroller.clone();
                let terminal = terminal.clone();
                move |cursor| {
                    scroller.set_cursor_from_name(cursor);
                    terminal.set_cursor_from_name(cursor);
                }
            },
        );

        let session_name = session_id.to_string();
        root.set_widget_name(&session_name);

        let label = gtk::Label::builder()
            .label(title)
            .ellipsize(pango::EllipsizeMode::End)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .xalign(0.0)
            .build();
        let close_icon = gtk::Image::from_icon_name("window-close-symbolic");
        close_icon.set_pixel_size(14);
        let close_button = gtk::Button::builder()
            .child(&close_icon)
            .tooltip_text("Close session")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        close_button.add_css_class("flat");
        close_button.add_css_class("circular");
        close_button.add_css_class("terminal-session-close-button");

        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&session_name);
        row.set_selectable(true);
        row.set_activatable(true);

        let row_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();
        row_content.append(&gtk::Image::from_icon_name("utilities-terminal-symbolic"));
        row_content.append(&label);
        row_content.append(&close_button);
        row.set_child(Some(&row_content));
        install_session_row_drag(session_id, &row, &self.sessions, &self.session_rail);

        self.session_stack.add_named(&root, Some(&session_name));
        self.session_stack.set_visible_child(&root);
        self.session_rail.append(&row);
        close_button.connect_clicked({
            let root = self.root.clone();
            let sessions = self.sessions.clone();
            let session_stack = self.session_stack.clone();
            let session_rail = self.session_rail.clone();
            let empty_handlers = self.empty_handlers.clone();

            move |_| {
                request_close_session(
                    session_id,
                    &root,
                    &sessions,
                    &session_stack,
                    &session_rail,
                    &empty_handlers,
                );
            }
        });

        let child_pid = Rc::new(Cell::new(None));
        let state = Rc::new(Cell::new(TerminalSessionState::Starting));
        let exit_success = Rc::new(Cell::new(false));
        let auto_close_source = Rc::new(RefCell::new(None));
        install_exit_key_handler(
            session_id,
            &self.root,
            &terminal,
            &state,
            &exit_success,
            &self.sessions,
            &self.session_stack,
            &self.session_rail,
            &self.empty_handlers,
            &auto_close_source,
        );
        install_terminal_interaction_trackers(
            session_id,
            &terminal,
            &state,
            &exit_success,
            &self.root,
            &self.sessions,
            &self.session_stack,
            &self.session_rail,
            &self.empty_handlers,
            &auto_close_source,
        );
        connect_child_exit(
            session_id,
            &terminal,
            &label,
            title,
            &child_pid,
            &state,
            &exit_success,
            &self.root,
            &self.sessions,
            &self.session_stack,
            &self.session_rail,
            &self.empty_handlers,
            &auto_close_source,
        );
        spawn_command(&terminal, command, &child_pid, &state)?;
        start_title_poll(&terminal, &label, title, &state);

        let session = TerminalSession {
            id: session_id,
            root,
            row,
            terminal,
            child_pid,
            launch_kind,
            state,
            exit_success,
            auto_close_source,
        };

        self.sessions.borrow_mut().push(session.clone());
        self.session_rail.select_row(Some(&session.row));
        session.terminal.grab_focus();
        Ok(session)
    }

    fn focus_active_terminal(&self) {
        focus_current_session_row(&self.sessions, &self.session_rail);
    }
}

fn install_session_row_drag(
    session_id: u64,
    row: &gtk::ListBoxRow,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_rail: &gtk::ListBox,
) {
    let drag = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .build();
    drag.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&session_id.to_value()))
    });
    row.add_controller(drag);

    let drop = gtk::DropTarget::new(u64::static_type(), gdk::DragAction::MOVE);
    drop.connect_drop({
        let row = row.clone();
        let sessions = sessions.clone();
        let session_rail = session_rail.clone();

        move |_, value, _, y| {
            let Ok(source_id) = value.get::<u64>() else {
                return false;
            };
            reorder_session_rows(&sessions, &session_rail, source_id, session_id, &row, y)
        }
    });
    row.add_controller(drop);
}

fn reorder_session_rows(
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_rail: &gtk::ListBox,
    source_id: u64,
    target_id: u64,
    target_row: &gtk::ListBoxRow,
    target_y: f64,
) -> bool {
    if source_id == target_id {
        return false;
    }

    let mut sessions_ref = sessions.borrow_mut();
    let Some(source_index) = sessions_ref
        .iter()
        .position(|session| session.id == source_id)
    else {
        return false;
    };
    let Some(target_index) = sessions_ref
        .iter()
        .position(|session| session.id == target_id)
    else {
        return false;
    };

    let insert_after = target_y > f64::from(target_row.allocated_height()) / 2.0;
    let mut insert_index = target_index + if insert_after { 1 } else { 0 };
    if source_index < insert_index {
        insert_index = insert_index.saturating_sub(1);
    }
    if source_index == insert_index {
        return false;
    }

    let session = sessions_ref.remove(source_index);
    let row = session.row.clone();
    sessions_ref.insert(insert_index, session);
    drop(sessions_ref);

    session_rail.remove(&row);
    session_rail.insert(&row, insert_index as i32);
    session_rail.select_row(Some(&row));
    true
}

fn configured_terminal(font_size: f64) -> vte4::Terminal {
    terminal_component::configured_terminal(font_size, DEFAULT_COLUMNS, DEFAULT_ROWS)
}

fn set_terminal_font(terminal: &vte4::Terminal, font_size: f64) {
    terminal_component::set_font(terminal, font_size);
}

fn install_terminal_shortcuts(
    terminal: &vte4::Terminal,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    search_panel: &SearchPanel,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let terminal = terminal.clone();
        let sessions = sessions.clone();
        let search_panel = search_panel.clone();

        move |_, key, _, modifiers| {
            if let Some(delta) = font_size_delta_for_key(key, modifiers) {
                let current = config::load().font_sizes.shell;
                let next =
                    config::normalize_font_size(current + delta, config::DEFAULT_SHELL_FONT_SIZE);
                if (next - current).abs() > f64::EPSILON {
                    set_terminal_font_for_sessions(&terminal, &sessions, next);
                    config::save_shell_font_size(next);
                }
                return glib::Propagation::Stop;
            }

            reset_kinetic_scroll_if_needed(&terminal, key);

            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

            if ctrl
                && !shift
                && !modifiers.contains(gdk::ModifierType::ALT_MASK)
                && matches!(key, gdk::Key::f | gdk::Key::F)
            {
                search_panel.open();
                return glib::Propagation::Stop;
            }

            if ctrl
                && !shift
                && matches!(key, gdk::Key::c | gdk::Key::C)
                && terminal.has_selection()
            {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::c | gdk::Key::C) {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::v | gdk::Key::V) {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && shift && key == gdk::Key::Insert {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if shift && key == gdk::Key::Insert {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && !shift && key == gdk::Key::BackSpace {
                terminal.feed_child(CTRL_BACKSPACE_SEQUENCE);
                return glib::Propagation::Stop;
            }

            if let Some(sequence) = modified_key_sequence(key, modifiers) {
                terminal.feed_child(sequence.as_bytes());
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }
    });
    terminal.add_controller(keys);
}

fn set_terminal_font_for_sessions(
    terminal: &vte4::Terminal,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    font_size: f64,
) {
    set_terminal_font(terminal, font_size);
    for session in sessions.borrow().iter() {
        set_terminal_font(&session.terminal, font_size);
    }
}

fn font_size_delta_for_key(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<f64> {
    if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        || modifiers.contains(gdk::ModifierType::ALT_MASK)
    {
        return None;
    }

    if key == gdk::Key::plus || key == gdk::Key::equal || key == gdk::Key::KP_Add {
        return Some(1.0);
    }
    if key == gdk::Key::minus || key == gdk::Key::underscore || key == gdk::Key::KP_Subtract {
        return Some(-1.0);
    }

    None
}

fn copy_terminal_selection(terminal: &vte4::Terminal) {
    context_menu::copy_terminal_selection(terminal);
}

fn reset_kinetic_scroll_if_needed(terminal: &vte4::Terminal, key: gdk::Key) {
    if is_modifier_key(key) || !terminal.is_scroll_on_keystroke() {
        return;
    }

    let Some(scroller) = terminal
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
    else {
        return;
    };
    if !scroller.is_kinetic_scrolling() {
        return;
    }

    let adjustment = scroller.vadjustment();
    if adjustment.upper() - adjustment.page_size() > adjustment.value() {
        scroller.set_kinetic_scrolling(false);
        scroller.set_kinetic_scrolling(true);
    }
}

fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::ISO_Level5_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
    )
}

fn install_terminal_activation(
    terminal: &vte4::Terminal,
    activation_handlers: &ActivationHandlers,
    launch_dir: String,
) {
    terminal_component::install_activation(terminal, launch_dir, {
        let activation_handlers = activation_handlers.clone();

        move |activation| {
            for handler in activation_handlers.borrow().iter() {
                handler(activation.clone());
            }
        }
    });
}

fn connect_terminal_search_option(
    search_panel: &SearchPanel,
    option: SearchOption,
    option_value: Rc<Cell<bool>>,
    sessions: Rc<RefCell<Vec<TerminalSession>>>,
    session_rail: gtk::ListBox,
    search_options: TerminalSearchOptions,
) {
    search_panel.connect_option_toggled(option, {
        let search_panel = search_panel.clone();

        move |active| {
            option_value.set(active);
            if let Some(terminal) = active_terminal(&sessions, &session_rail) {
                apply_terminal_search(
                    &terminal,
                    &search_panel,
                    &search_options,
                    TerminalSearchMove::Next,
                );
            }
        }
    });
}

fn active_terminal(
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_rail: &gtk::ListBox,
) -> Option<vte4::Terminal> {
    session_rail
        .selected_row()
        .and_then(|row| session_by_row(sessions, &row))
        .map(|session| session.terminal)
}

fn apply_terminal_search(
    terminal: &vte4::Terminal,
    search_panel: &SearchPanel,
    options: &TerminalSearchOptions,
    search_move: TerminalSearchMove,
) {
    let query = search_panel.query();
    if query.is_empty() {
        terminal.search_set_regex(None, 0);
        search_panel.set_status("");
        return;
    }

    let pattern = terminal_search_pattern(&query, options);
    let regex = match vte4::Regex::for_search(&pattern, 0) {
        Ok(regex) => regex,
        Err(err) => {
            terminal.search_set_regex(None, 0);
            search_panel.set_status("Invalid");
            log::warn!(
                "terminal search regex invalid query_len={} regex_mode={}: {err}",
                query.len(),
                options.regex.get()
            );
            return;
        }
    };

    terminal.search_set_regex(Some(&regex), 0);
    terminal.search_set_wrap_around(true);
    let found = match search_move {
        TerminalSearchMove::Previous => terminal.search_find_previous(),
        TerminalSearchMove::Keep | TerminalSearchMove::Next => terminal.search_find_next(),
    };
    search_panel.set_status(if found { "Found" } else { "No Results" });
    log::debug!(
        "terminal search applied query_len={} move={search_move:?} found={found}",
        query.len()
    );
}

fn terminal_search_pattern(query: &str, options: &TerminalSearchOptions) -> String {
    let mut pattern = if options.regex.get() {
        query.to_string()
    } else {
        regex::escape(query)
    };

    if options.whole_word.get() {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    if !options.case_sensitive.get() {
        pattern = format!("(?i:{pattern})");
    }

    pattern
}

fn modified_key_sequence(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<String> {
    let modifier_mask = kitty_modifier_mask(modifiers)?;
    let codepoint = modified_key_codepoint(key)?;
    Some(format!("\x1b[{codepoint};{modifier_mask}:1u"))
}

fn kitty_modifier_mask(modifiers: gdk::ModifierType) -> Option<u8> {
    let mut mask = 1;

    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        mask += 1;
    }
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        mask += 2;
    }
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        mask += 4;
    }
    if modifiers.contains(gdk::ModifierType::SUPER_MASK) {
        mask += 8;
    }

    (mask != 1).then_some(mask)
}

fn modified_key_codepoint(key: gdk::Key) -> Option<u32> {
    match key {
        gdk::Key::Return | gdk::Key::KP_Enter => Some(13),
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Some(9),
        gdk::Key::BackSpace => Some(127),
        gdk::Key::Escape => Some(27),
        gdk::Key::Left => Some(57_417),
        gdk::Key::Right => Some(57_418),
        gdk::Key::Up => Some(57_419),
        gdk::Key::Down => Some(57_420),
        gdk::Key::Page_Up => Some(57_421),
        gdk::Key::Page_Down => Some(57_422),
        gdk::Key::Home => Some(57_423),
        gdk::Key::End => Some(57_424),
        gdk::Key::Insert => Some(57_425),
        gdk::Key::Delete => Some(57_426),
        _ => None,
    }
}

fn install_focus_tracking(terminal: &vte4::Terminal, focus_handlers: &FocusHandlers) {
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter({
        let focus_handlers = focus_handlers.clone();

        move |_| notify_focus_handlers(&focus_handlers, true)
    });
    focus.connect_leave({
        let focus_handlers = focus_handlers.clone();

        move |_| notify_focus_handlers(&focus_handlers, false)
    });
    terminal.add_controller(focus);
}

fn notify_focus_handlers(focus_handlers: &FocusHandlers, focused: bool) {
    for handler in focus_handlers.borrow().iter() {
        handler(focused);
    }
}

fn install_terminal_interaction_trackers(
    session_id: u64,
    terminal: &vte4::Terminal,
    state: &Rc<Cell<TerminalSessionState>>,
    exit_success: &Rc<Cell<bool>>,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
    auto_close_source: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    let reset_auto_close = {
        let root = root.clone();
        let state = state.clone();
        let exit_success = exit_success.clone();
        let sessions = sessions.clone();
        let session_stack = session_stack.clone();
        let session_rail = session_rail.clone();
        let empty_handlers = empty_handlers.clone();
        let auto_close_source = auto_close_source.clone();

        move || {
            queue_terminal_auto_close(
                session_id,
                &root,
                &state,
                &exit_success,
                &sessions,
                &session_stack,
                &session_rail,
                &empty_handlers,
                &auto_close_source,
            );
        }
    };

    let click = gtk::GestureClick::builder().button(0).build();
    click.connect_pressed({
        let reset_auto_close = reset_auto_close.clone();

        move |_, _, _, _| {
            reset_auto_close();
        }
    });
    terminal.add_controller(click);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    scroll.connect_scroll({
        let reset_auto_close = reset_auto_close.clone();

        move |_, _, _| {
            reset_auto_close();
            gtk::glib::Propagation::Proceed
        }
    });
    terminal.add_controller(scroll);
}

fn queue_terminal_auto_close(
    session_id: u64,
    root: &gtk::Box,
    state: &Rc<Cell<TerminalSessionState>>,
    exit_success: &Rc<Cell<bool>>,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
    auto_close_source: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    if state.get() != TerminalSessionState::Exited || !exit_success.get() {
        return;
    }

    if let Some(source) = auto_close_source.borrow_mut().take() {
        source.remove();
    }

    let root = root.clone();
    let state = state.clone();
    let exit_success = exit_success.clone();
    let sessions = sessions.clone();
    let session_stack = session_stack.clone();
    let session_rail = session_rail.clone();
    let empty_handlers = empty_handlers.clone();
    let auto_close_source_for_timer = auto_close_source.clone();
    let auto_close_source_for_slot = auto_close_source.clone();

    let source_id = glib::timeout_add_local(
        std::time::Duration::from_secs(TERMINAL_AUTO_CLOSE_IDLE_SECONDS),
        move || {
            let should_close = {
                let sessions_ref = sessions.borrow();
                sessions_ref.iter().any(|session| {
                    session.id == session_id
                        && session.state.get() == TerminalSessionState::Exited
                        && session.exit_success.get()
                }) && state.get() == TerminalSessionState::Exited
                    && exit_success.get()
            };

            if !should_close {
                auto_close_source_for_timer.borrow_mut().take();
                return glib::ControlFlow::Break;
            }

            auto_close_source_for_timer.borrow_mut().take();
            log::info!(
                "auto-closing terminal session_id={} after {}s of inactivity (exit code 0)",
                session_id,
                TERMINAL_AUTO_CLOSE_IDLE_SECONDS
            );
            close_session(
                session_id,
                &root,
                &sessions,
                &session_stack,
                &session_rail,
                &empty_handlers,
            );
            glib::ControlFlow::Break
        },
    );
    auto_close_source_for_slot.replace(Some(source_id));
    log::debug!(
        "scheduled terminal auto-close session_id={} in {}s if no interaction",
        session_id,
        TERMINAL_AUTO_CLOSE_IDLE_SECONDS
    );
}

fn clear_terminal_auto_close_timer(auto_close_source: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(source) = auto_close_source.borrow_mut().take() {
        source.remove();
    }
}

fn install_exit_key_handler(
    session_id: u64,
    root: &gtk::Box,
    terminal: &vte4::Terminal,
    state: &Rc<Cell<TerminalSessionState>>,
    exit_success: &Rc<Cell<bool>>,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
    auto_close_source: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let root = root.clone();
        let state = state.clone();
        let exit_success = exit_success.clone();
        let sessions = sessions.clone();
        let session_stack = session_stack.clone();
        let session_rail = session_rail.clone();
        let empty_handlers = empty_handlers.clone();
        let auto_close_source = auto_close_source.clone();

        move |_, key, _, _| {
            if state.get() == TerminalSessionState::Exited
                && exit_success.get()
                && key != gdk::Key::Return
                && key != gdk::Key::KP_Enter
            {
                queue_terminal_auto_close(
                    session_id,
                    &root,
                    &state,
                    &exit_success,
                    &sessions,
                    &session_stack,
                    &session_rail,
                    &empty_handlers,
                    &auto_close_source,
                );
            }

            if state.get() == TerminalSessionState::Exited
                && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
            {
                close_session(
                    session_id,
                    &root,
                    &sessions,
                    &session_stack,
                    &session_rail,
                    &empty_handlers,
                );
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }
    });
    terminal.add_controller(keys);
}

fn spawn_command(
    terminal: &vte4::Terminal,
    command: &CommandSpec,
    child_pid: &Rc<Cell<Option<glib::Pid>>>,
    state: &Rc<Cell<TerminalSessionState>>,
) -> Result<(), String> {
    let argv = command_argv(command)?;
    let argv_refs = argv.iter().map(String::as_str).collect::<Vec<_>>();
    let env = terminal_environment();
    let env_refs = env.iter().map(String::as_str).collect::<Vec<_>>();
    let working_dir = command.working_dir.to_str().ok_or_else(|| {
        format!(
            "Cannot start {}: working directory is not valid UTF-8.",
            command.display()
        )
    })?;

    terminal.spawn_async(
        vte4::PtyFlags::DEFAULT,
        Some(working_dir),
        &argv_refs,
        &env_refs,
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&gio::Cancellable>,
        {
            let terminal = terminal.clone();
            let child_pid = child_pid.clone();
            let state = state.clone();
            let display = command.display();

            move |result| match result {
                Ok(pid) => {
                    if state.get() == TerminalSessionState::Closing {
                        terminate_child(Some(pid));
                        return;
                    }

                    child_pid.set(Some(pid));
                    state.set(TerminalSessionState::Running);
                }
                Err(err) => {
                    if state.get() == TerminalSessionState::Closing {
                        return;
                    }

                    child_pid.set(None);
                    state.set(TerminalSessionState::Exited);
                    let message = format!(
                        "Failed to start {display}: {err}\r\n\r\nPress Enter to close the terminal.\r\n"
                    );
                    terminal.feed(message.as_bytes());
                }
            }
        },
    );
    Ok(())
}

fn local_spawn_dir_for_shell_command(command: &ShellCommandSpec) -> PathBuf {
    let target_dir = PathBuf::from(&command.working_dir.absolute);
    if target_dir.is_dir() {
        return target_dir;
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn command_argv(command: &CommandSpec) -> Result<Vec<String>, String> {
    command
        .argv()
        .into_iter()
        .map(|part| {
            part.into_string().map_err(|part| {
                format!(
                    "Cannot start {}: argument is not valid UTF-8: {}",
                    command.display(),
                    part.to_string_lossy()
                )
            })
        })
        .collect()
}

fn terminal_environment() -> Vec<String> {
    let mut env = std::env::vars().collect::<Vec<_>>();
    set_env(&mut env, "TERM", TERM_NAME);
    set_env(&mut env, "COLORTERM", COLORTERM_NAME);
    set_env(&mut env, "TERM_PROGRAM", "VTE");
    set_env(&mut env, "TERM_PROGRAM_VERSION", VTE_VERSION);
    set_env(&mut env, "VTE_VERSION", VTE_VERSION);

    if !has_utf8_locale(&env) {
        set_env(&mut env, "LC_CTYPE", "C.UTF-8");
    }

    env.into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    env.retain(|(existing, _)| existing != key);
    env.push((key.to_string(), value.to_string()));
}

fn has_utf8_locale(env: &[(String, String)]) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|key| {
        env.iter().any(|(existing_key, value)| {
            existing_key == key && {
                let value = value.to_ascii_lowercase();
                value.contains("utf-8") || value.contains("utf8")
            }
        })
    })
}

fn start_title_poll(
    terminal: &vte4::Terminal,
    label: &gtk::Label,
    fallback_title: &str,
    state: &Rc<Cell<TerminalSessionState>>,
) {
    update_session_title(label, &short_terminal_title(terminal, fallback_title));

    terminal.connect_termprop_changed(None, {
        let label = label.clone();
        let fallback_title = fallback_title.to_string();

        move |terminal, _| {
            update_session_title(&label, &short_terminal_title(terminal, &fallback_title));
        }
    });

    glib::timeout_add_local(std::time::Duration::from_millis(500), {
        let terminal = terminal.clone();
        let label = label.clone();
        let fallback_title = fallback_title.to_string();
        let state = state.clone();

        move || {
            if matches!(
                state.get(),
                TerminalSessionState::Exited | TerminalSessionState::Closing
            ) {
                return glib::ControlFlow::Break;
            }

            update_session_title(&label, &short_terminal_title(&terminal, &fallback_title));
            glib::ControlFlow::Continue
        }
    });
}

fn connect_child_exit(
    session_id: u64,
    terminal: &vte4::Terminal,
    label: &gtk::Label,
    fallback_title: &str,
    child_pid: &Rc<Cell<Option<glib::Pid>>>,
    state: &Rc<Cell<TerminalSessionState>>,
    exit_success: &Rc<Cell<bool>>,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
    auto_close_source: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    terminal.connect_child_exited({
        let label = label.clone();
        let fallback_title = fallback_title.to_string();
        let child_pid = child_pid.clone();
        let state = state.clone();
        let exit_success = exit_success.clone();
        let root = root.clone();
        let sessions = sessions.clone();
        let session_stack = session_stack.clone();
        let session_rail = session_rail.clone();
        let empty_handlers = empty_handlers.clone();
        let auto_close_source = auto_close_source.clone();

        move |terminal, status| {
            child_pid.set(None);
            if state.get() == TerminalSessionState::Closing {
                return;
            }

            state.set(TerminalSessionState::Exited);
            let success = status_exit_succeeded(status);
            exit_success.set(success);
            let summary = child_exit_summary(status);
            terminal.feed(
                format!(
                    "\r\n\r\nProgram {}. Press Enter to close the terminal.\r\n",
                    summary.message
                )
                .as_bytes(),
            );
            label.set_label(&format!("{fallback_title} ({})", summary.label));

            if success {
                log::info!(
                    "terminal exited with code 0; scheduling auto-close in {}s if no interaction",
                    TERMINAL_AUTO_CLOSE_IDLE_SECONDS
                );
                queue_terminal_auto_close(
                    session_id,
                    &root,
                    &state,
                    &exit_success,
                    &sessions,
                    &session_stack,
                    &session_rail,
                    &empty_handlers,
                    &auto_close_source,
                );
            }
        }
    });
}

fn status_exit_succeeded(status: i32) -> bool {
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

struct ChildExitSummary {
    message: String,
    label: String,
}

fn child_exit_summary(status: i32) -> ChildExitSummary {
    if libc::WIFEXITED(status) {
        let code = libc::WEXITSTATUS(status);
        return ChildExitSummary {
            message: format!("exited with code {code}"),
            label: format!("exited {code}"),
        };
    }

    if libc::WIFSIGNALED(status) {
        let signal = libc::WTERMSIG(status);
        return ChildExitSummary {
            message: format!("terminated by signal {signal}"),
            label: format!("signal {signal}"),
        };
    }

    ChildExitSummary {
        message: format!("exited with status {status}"),
        label: "exited".to_string(),
    }
}

fn short_terminal_title(terminal: &vte4::Terminal, fallback_title: &str) -> String {
    terminal_window_title(terminal)
        .or_else(|| foreground_process_name(terminal))
        .unwrap_or_else(|| fallback_title.to_string())
}

fn update_session_title(label: &gtk::Label, title: &str) {
    label.set_label(title);
}

fn terminal_window_title(terminal: &vte4::Terminal) -> Option<String> {
    let title = terminal.property::<Option<glib::GString>>("window-title")?;
    terminal_title_text(title.as_str())
}

fn terminal_title_text(title: &str) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn foreground_process_name(terminal: &vte4::Terminal) -> Option<String> {
    let foreground_pgid = foreground_process_group(terminal)?;
    process_name_for_group(foreground_pgid)
}

fn foreground_process_group(terminal: &vte4::Terminal) -> Option<libc::pid_t> {
    let fd = terminal.pty()?.fd().as_raw_fd();
    let foreground_pgid = unsafe { libc::tcgetpgrp(fd) };
    if foreground_pgid <= 0 {
        return None;
    }
    Some(foreground_pgid)
}

fn process_name_for_group(foreground_pgid: libc::pid_t) -> Option<String> {
    process_name(foreground_pgid).or_else(|| {
        fs::read_dir("/proc")
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
            .find(|pid| process_group(*pid) == Some(foreground_pgid))
            .and_then(process_name)
    })
}

fn process_name(pid: libc::pid_t) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn process_group(pid: libc::pid_t) -> Option<libc::pid_t> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    fields.get(2)?.parse().ok()
}

fn focus_current_session_row(
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_rail: &gtk::ListBox,
) {
    if let Some(session) = session_rail
        .selected_row()
        .and_then(|row| session_by_row(sessions, &row))
    {
        session.terminal.grab_focus();
    }
}

fn close_session(
    session_id: u64,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
) {
    let session = {
        let mut sessions = sessions.borrow_mut();
        let Some(index) = sessions.iter().position(|session| session.id == session_id) else {
            return;
        };
        sessions.remove(index)
    };

    clear_terminal_auto_close_timer(&session.auto_close_source);
    session.state.set(TerminalSessionState::Closing);
    let child_pid = session.child_pid.get();
    session.child_pid.set(None);
    terminate_child(child_pid);
    session_stack.remove(&session.root);
    session_rail.remove(&session.row);

    if sessions.borrow().is_empty() {
        root.set_visible(false);
        for handler in empty_handlers.borrow().iter() {
            handler();
        }
    } else if let Some(next_session) = session_rail
        .selected_row()
        .and_then(|row| session_by_row(sessions, &row))
        .or_else(|| sessions.borrow().last().cloned())
    {
        session_stack.set_visible_child(&next_session.root);
        session_rail.select_row(Some(&next_session.row));
        next_session.terminal.grab_focus();
    }
}

fn session_by_id(
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_id: u64,
) -> Option<TerminalSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
}

fn session_by_row(
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    row: &gtk::ListBoxRow,
) -> Option<TerminalSession> {
    row.widget_name()
        .parse::<u64>()
        .ok()
        .and_then(|session_id| session_by_id(sessions, session_id))
}

fn request_close_session(
    session_id: u64,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
) {
    let Some(session) = session_by_id(sessions, session_id) else {
        return;
    };

    let Some(task_name) = active_task_name(&session) else {
        close_session(
            session_id,
            root,
            sessions,
            session_stack,
            session_rail,
            empty_handlers,
        );
        return;
    };

    confirm_close_running_task(
        &task_name,
        root,
        session_id,
        sessions,
        session_stack,
        session_rail,
        empty_handlers,
    );
}

fn active_task_name(session: &TerminalSession) -> Option<String> {
    if session.state.get() != TerminalSessionState::Running {
        return None;
    }

    let child_pid = session.child_pid.get()?;
    match session.launch_kind {
        TerminalLaunchKind::Command => foreground_process_name(&session.terminal)
            .or_else(|| process_name(child_pid.0 as libc::pid_t))
            .or_else(|| Some("The process".to_string())),
        TerminalLaunchKind::Shell => active_shell_task_name(&session.terminal, child_pid),
    }
}

fn active_shell_task_name(terminal: &vte4::Terminal, shell_pid: glib::Pid) -> Option<String> {
    let foreground_pgid = foreground_process_group(terminal)?;
    let shell_pid = shell_pid.0 as libc::pid_t;
    let shell_pgid = process_group(shell_pid).unwrap_or(shell_pid);

    if foreground_pgid == shell_pgid {
        return None;
    }

    process_name_for_group(foreground_pgid).or_else(|| Some("The foreground process".to_string()))
}

fn confirm_close_running_task(
    task_name: &str,
    root: &gtk::Box,
    session_id: u64,
    sessions: &Rc<RefCell<Vec<TerminalSession>>>,
    session_stack: &gtk::Stack,
    session_rail: &gtk::ListBox,
    empty_handlers: &EmptyHandlers,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Close Running Terminal?")
        .body(&format!(
            "{task_name} is still running. Closing this terminal will terminate it."
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("close", "Close Terminal");
    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = root
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, {
        let root = root.clone();
        let sessions = sessions.clone();
        let session_stack = session_stack.clone();
        let session_rail = session_rail.clone();
        let empty_handlers = empty_handlers.clone();

        move |response| {
            if response.as_str() != "close" {
                return;
            }

            close_session(
                session_id,
                &root,
                &sessions,
                &session_stack,
                &session_rail,
                &empty_handlers,
            );
        }
    });
}

fn terminate_child(pid: Option<glib::Pid>) {
    let Some(pid) = pid else {
        return;
    };
    unsafe {
        libc::kill(pid.0 as libc::pid_t, libc::SIGHUP);
    }
}
