use crate::process::signal_terminal_process_groups;
use crate::ui::components::terminal::{TerminalActivation, TerminalFileActivation};
use craic_ui_core::ui::{canvas_scroll, components::context_menu};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::rc::Rc;
use vte4::prelude::*;

const DEFAULT_COLUMNS: i64 = 100;
const DEFAULT_ROWS: i64 = 34;
const SCROLLBACK_LINES: i64 = 10_000;
const VTE_VERSION: &str = "8400";
const TERMINAL_URL_MATCH_PATTERN: &str = r#"https?://[^\s<>\"'`]+"#;
const TERMINAL_PATH_MATCH_PATTERN: &str = r#"(?x)
    (?:
        /[A-Za-z0-9._~+%/@=-]+
        |
        (?:\.{1,2}/)?(?:[A-Za-z0-9._~+%-]+/)+[A-Za-z0-9._~+%-]+
        |
        [A-Za-z0-9._~+%-]+\.[A-Za-z0-9._~+%-]+
    )
    (?::[0-9]+(?::[0-9]+)?)?
"#;

type ActivationHandlers = Rc<RefCell<Vec<Box<dyn Fn(TerminalActivation)>>>>;

#[derive(Clone)]
pub struct VteTerminal {
    root: gtk::Overlay,
    terminal: vte4::Terminal,
    scroller: gtk::ScrolledWindow,
    child_pid: Rc<Cell<Option<i32>>>,
    spawning: Rc<Cell<bool>>,
    terminated: Rc<Cell<bool>>,
    launch_dir: Rc<RefCell<String>>,
    activation_handlers: ActivationHandlers,
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub env: HashMap<String, String>,
}

pub fn terminal_environment() -> HashMap<String, String> {
    let mut environment = std::env::vars().collect::<HashMap<_, _>>();
    environment.insert("TERM".to_string(), "xterm-256color".to_string());
    environment.insert("COLORTERM".to_string(), "truecolor".to_string());
    environment.insert("TERM_PROGRAM".to_string(), "Craic".to_string());
    environment.insert("VTE_VERSION".to_string(), VTE_VERSION.to_string());
    if !["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|key| {
        environment.get(key).is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
    }) {
        environment.insert("LC_CTYPE".to_string(), "C.UTF-8".to_string());
    }
    environment
}

impl VteTerminal {
    pub fn new(font_size: f64) -> Self {
        let terminal = vte4::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_focusable(true);
        terminal.set_size(DEFAULT_COLUMNS, DEFAULT_ROWS);
        terminal.set_scrollback_lines(SCROLLBACK_LINES);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_scroll_on_output(false);
        terminal.set_scroll_unit_is_pixels(true);
        terminal.set_enable_fallback_scrolling(false);
        terminal.set_mouse_autohide(true);
        terminal.set_bold_is_bright(true);
        terminal.set_enable_sixel(true);
        terminal.set_allow_hyperlink(true);
        terminal.set_enable_shaping(false);
        terminal.set_enable_bidi(false);
        set_font(&terminal, font_size);
        terminal.set_colors(
            Some(&rgba(212, 212, 212)),
            Some(&rgba(30, 30, 30)),
            &ansi_palette().iter().collect::<Vec<_>>(),
        );
        terminal.set_color_cursor(Some(&rgba(174, 175, 173)));
        terminal.set_color_highlight(Some(&rgba(38, 79, 120)));
        terminal.set_color_highlight_foreground(Some(&rgba(255, 255, 255)));

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&terminal)
            .build();
        let marker = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&scroller));
        root.add_overlay(&marker);

        let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
        canvas_scroll::install_scrolled_window_middle_autoscroll_with_state(
            &scroller,
            &marker,
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

        let child_pid = Rc::new(Cell::new(None));
        terminal.connect_child_exited({
            let child_pid = child_pid.clone();
            move |_, _| child_pid.set(None)
        });
        terminal.connect_selection_changed(|terminal| {
            if terminal.has_selection() {
                terminal.copy_primary();
            }
        });

        let this = Self {
            root,
            terminal,
            scroller,
            child_pid,
            spawning: Rc::new(Cell::new(false)),
            terminated: Rc::new(Cell::new(false)),
            launch_dir: Rc::new(RefCell::new(String::new())),
            activation_handlers: Rc::new(RefCell::new(Vec::new())),
        };
        this.install_matches();
        this.install_activation();
        this.install_context_menu();
        this.install_file_drop();
        this
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        &self.terminal
    }

    pub fn grab_focus(&self) {
        self.terminal.grab_focus();
    }

    pub fn spawn<F>(&self, spec: SpawnSpec, launch_dir: String, callback: F) -> Result<(), String>
    where
        F: FnOnce(Result<i32, String>) + 'static,
    {
        if self.spawning.get() || self.child_pid.get().is_some() {
            return Err("Terminal process has already been started.".to_string());
        }
        self.spawning.set(true);
        let working_directory = spec.working_directory.to_str().ok_or_else(|| {
            self.spawning.set(false);
            "Terminal working directory is not valid UTF-8.".to_string()
        })?;
        if spec.program.is_empty() {
            self.spawning.set(false);
            return Err("Cannot start an empty terminal command.".to_string());
        }
        self.terminated.set(false);
        self.launch_dir.replace(launch_dir);

        let argv = std::iter::once(spec.program)
            .chain(spec.args)
            .collect::<Vec<_>>();
        let argv_refs = argv.iter().map(String::as_str).collect::<Vec<_>>();
        let env = spec
            .env
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>();
        let env_refs = env.iter().map(String::as_str).collect::<Vec<_>>();
        self.terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(working_directory),
            &argv_refs,
            &env_refs,
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gio::Cancellable>,
            {
                let this = self.clone();
                move |result| {
                    this.spawning.set(false);
                    match result {
                        Ok(pid) => {
                            this.child_pid.set(Some(pid.0));
                            log::info!("VTE terminal PTY started pid={}", pid.0);
                            if this.terminated.get() {
                                signal_terminal_process_groups(
                                    pid.0,
                                    this.foreground_process_group(),
                                    libc::SIGHUP,
                                );
                                this.child_pid.set(None);
                            }
                            callback(Ok(pid.0));
                        }
                        Err(err) => callback(Err(err.to_string())),
                    }
                }
            },
        );
        Ok(())
    }

    pub fn connect_child_exited<F: Fn(ExitStatus) + 'static>(&self, callback: F) {
        self.terminal.connect_child_exited(move |_, status| {
            callback(ExitStatus::from_raw(status));
        });
    }

    pub fn connect_title_changed<F: Fn(String) + 'static>(&self, callback: F) {
        self.terminal
            .connect_notify_local(Some("window-title"), move |terminal, _| {
                callback(
                    terminal
                        .property::<Option<glib::GString>>("window-title")
                        .map(|title| title.to_string())
                        .unwrap_or_default(),
                );
            });
    }

    pub fn connect_activation<F: Fn(TerminalActivation) + 'static>(&self, callback: F) {
        self.activation_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }

    pub fn title(&self) -> Option<String> {
        self.terminal
            .property::<Option<glib::GString>>("window-title")
            .map(|title| title.to_string())
    }

    pub fn foreground_process_group(&self) -> Option<libc::pid_t> {
        let pty = self.terminal.pty()?;
        let pgid = unsafe { libc::tcgetpgrp(pty.fd().as_raw_fd()) };
        (pgid > 0).then_some(pgid)
    }

    pub fn terminate(&self) {
        self.terminated.set(true);
        let Some(pid) = self.child_pid.take() else {
            log::debug!("VTE terminal shutdown deferred or already complete");
            return;
        };
        log::info!(
            "VTE terminal shutdown started pid={pid} foreground_pgid={:?}",
            self.foreground_process_group()
        );
        signal_terminal_process_groups(pid, self.foreground_process_group(), libc::SIGHUP);
    }

    pub fn set_font_size(&self, font_size: f64) {
        set_font(&self.terminal, font_size);
    }

    pub fn has_selection(&self) -> bool {
        self.terminal.has_selection()
    }

    pub fn copy_clipboard(&self) {
        self.terminal.copy_clipboard_format(vte4::Format::Text);
    }

    pub fn paste_clipboard(&self) {
        self.terminal.paste_clipboard();
    }

    pub fn paste_text(&self, text: &str) {
        self.terminal.paste_text(text);
    }

    pub fn feed_child(&self, bytes: &[u8]) {
        self.terminal.feed_child(bytes);
    }

    pub fn feed(&self, bytes: &[u8]) {
        self.terminal.feed(bytes);
    }

    pub fn search(&self, pattern: Option<&str>, backwards: bool) -> Result<bool, String> {
        let Some(pattern) = pattern else {
            self.terminal.search_set_regex(None, 0);
            return Ok(false);
        };
        let regex = vte4::Regex::for_search(pattern, 0).map_err(|err| err.to_string())?;
        self.terminal.search_set_regex(Some(&regex), 0);
        self.terminal.search_set_wrap_around(true);
        Ok(if backwards {
            self.terminal.search_find_previous()
        } else {
            self.terminal.search_find_next()
        })
    }

    pub fn all_text(&self) -> Option<String> {
        let (_, cursor_row) = self.terminal.cursor_position();
        let end_row = cursor_row.max(self.terminal.row_count().saturating_sub(1));
        self.text_range_between(0, end_row)
    }

    pub fn cursor_row(&self) -> Option<i64> {
        Some(self.terminal.cursor_position().1)
    }

    pub fn text_before_cursor(&self, max_lines: usize) -> Option<String> {
        let (_, cursor_row) = self.terminal.cursor_position();
        if cursor_row <= 0 {
            return None;
        }
        let end_row = cursor_row - 1;
        let start_row = (end_row - max_lines.saturating_sub(1) as i64).max(0);
        self.text_range_between(start_row, end_row)
    }

    pub fn recent_text(&self, max_lines: usize) -> Option<String> {
        let (_, cursor_row) = self.terminal.cursor_position();
        let end_row = cursor_row
            .saturating_sub(1)
            .max(self.terminal.row_count().saturating_sub(1));
        let start_row = (end_row - max_lines.saturating_sub(1) as i64).max(0);
        self.text_range_between(start_row, end_row)
    }

    fn text_range_between(&self, start_row: i64, end_row: i64) -> Option<String> {
        let end_col = self.terminal.column_count().max(1);
        let (text, _) =
            self.terminal
                .text_range_format(vte4::Format::Text, start_row, 0, end_row, end_col);
        text.map(|text| text.to_string())
    }

    fn visible_text(&self) -> Option<String> {
        let adjustment = self.scroller.vadjustment();
        let char_height = self.terminal.char_height().max(1) as f64;
        let start_row = (adjustment.value() / char_height).floor().max(0.0) as i64;
        let visible_rows = (adjustment.page_size() / char_height).ceil().max(1.0) as i64;
        self.text_range_between(start_row, start_row + visible_rows)
    }

    fn install_matches(&self) {
        for (name, pattern) in [
            ("url", TERMINAL_URL_MATCH_PATTERN),
            ("path", TERMINAL_PATH_MATCH_PATTERN),
        ] {
            match vte4::Regex::for_match(pattern, 0) {
                Ok(regex) => {
                    let tag = self.terminal.match_add_regex(&regex, 0);
                    self.terminal.match_set_cursor_name(tag, "pointer");
                    log::debug!("terminal match regex installed kind={name} tag={tag}");
                }
                Err(err) => log::warn!("terminal match regex failed kind={name}: {err}"),
            }
        }
    }

    fn install_activation(&self) {
        let click = gtk::GestureClick::builder().button(1).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let this = self.clone();
            move |gesture, press_count, x, y| {
                let modifiers = gesture.current_event_state();
                if press_count != 1
                    || !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                    || modifiers.contains(gdk::ModifierType::ALT_MASK)
                {
                    return;
                }
                let Some(target) = this
                    .terminal
                    .check_hyperlink_at(x, y)
                    .or_else(|| this.terminal.check_match_at(x, y).0)
                    .and_then(|value| clean_activation_text(value.as_str()))
                else {
                    return;
                };
                let activation = if is_http_url(&target) {
                    TerminalActivation::Url(target)
                } else {
                    TerminalActivation::File(TerminalFileActivation {
                        target,
                        launch_dir: this.launch_dir.borrow().clone(),
                    })
                };
                for handler in this.activation_handlers.borrow().iter() {
                    handler(activation.clone());
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.terminal.add_controller(click);
    }

    fn install_context_menu(&self) {
        #[derive(Clone, Copy, Debug)]
        enum Action {
            Copy,
            CopyScreen,
            CopyAll,
            SelectAll,
            Paste,
        }

        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let this = self.clone();
            move |gesture, _, x, y| {
                if gesture.current_button() != 3 {
                    return;
                }
                let popover = context_menu::popup_action_menu(
                    &this.terminal,
                    x,
                    y,
                    vec![
                        context_menu::ActionMenuSection::new(vec![
                            context_menu::ActionMenuItem::new(
                                "Copy",
                                Action::Copy,
                                this.has_selection(),
                            ),
                            context_menu::ActionMenuItem::new(
                                "Copy Screen",
                                Action::CopyScreen,
                                true,
                            ),
                            context_menu::ActionMenuItem::new("Copy All", Action::CopyAll, true),
                        ]),
                        context_menu::ActionMenuSection::new(vec![
                            context_menu::ActionMenuItem::new(
                                "Select All",
                                Action::SelectAll,
                                true,
                            ),
                            context_menu::ActionMenuItem::new("Paste", Action::Paste, true),
                        ]),
                    ],
                    {
                        let this = this.clone();
                        move |action| {
                            match action {
                                Action::Copy => this.copy_clipboard(),
                                Action::CopyScreen => {
                                    if let Some(text) = this.visible_text() {
                                        this.terminal.clipboard().set_text(&text);
                                    }
                                }
                                Action::CopyAll => {
                                    if let Some(text) = this.all_text() {
                                        this.terminal.clipboard().set_text(&text);
                                    }
                                }
                                Action::SelectAll => this.terminal.select_all(),
                                Action::Paste => this.paste_clipboard(),
                            }
                            this.grab_focus();
                            log::debug!("terminal context menu action activated action={action:?}");
                        }
                    },
                );
                popover.connect_closed(|popover| {
                    let popover = popover.clone();
                    glib::idle_add_local_once(move || popover.unparent());
                });
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.terminal.add_controller(click);
    }

    fn install_file_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop({
            let this = self.clone();
            move |_, value, _, _| {
                let Ok(files) = value.get::<gdk::FileList>() else {
                    return false;
                };
                let paths = files
                    .files()
                    .into_iter()
                    .filter_map(|file| file.path())
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return false;
                }
                let text = paths
                    .iter()
                    .map(|path| shell_quote(&path.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(" ");
                this.paste_text(&text);
                this.grab_focus();
                log::info!("VTE terminal file drop pasted paths count={}", paths.len());
                true
            }
        });
        self.terminal.add_controller(target);
    }
}

fn set_font(terminal: &vte4::Terminal, font_size: f64) {
    terminal.set_font(Some(&pango::FontDescription::from_string(&format!(
        "monospace {}",
        font_size.round() as i32
    ))));
}

fn clean_activation_text(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.trim_end_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ')' | ']' | '}'));
    (!value.is_empty()).then(|| value.to_string())
}

fn is_http_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn ansi_palette() -> [gdk::RGBA; 16] {
    [
        rgba(0, 0, 0),
        rgba(205, 49, 49),
        rgba(13, 188, 121),
        rgba(229, 229, 16),
        rgba(36, 114, 200),
        rgba(188, 63, 188),
        rgba(17, 168, 205),
        rgba(229, 229, 229),
        rgba(102, 102, 102),
        rgba(241, 76, 76),
        rgba(35, 209, 139),
        rgba(245, 245, 67),
        rgba(59, 142, 234),
        rgba(214, 112, 214),
        rgba(41, 184, 219),
        rgba(255, 255, 255),
    ]
}

fn rgba(red: u8, green: u8, blue: u8) -> gdk::RGBA {
    gdk::RGBA::new(
        red as f32 / 255.0,
        green as f32 / 255.0,
        blue as f32 / 255.0,
        1.0,
    )
}
