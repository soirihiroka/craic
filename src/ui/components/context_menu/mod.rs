use adw::prelude::*;
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vte4::prelude::*;

#[derive(Clone)]
pub(crate) struct ActionMenuItem<A> {
    pub label: String,
    pub action: A,
    pub enabled: bool,
}

#[derive(Clone)]
pub(crate) struct ActionMenuSection<A> {
    pub items: Vec<ActionMenuItem<A>>,
}

#[derive(Clone, Copy)]
pub(crate) struct MenuActionState {
    pub visible: bool,
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TextContextAction {
    CorrectSpelling {
        start: usize,
        end: usize,
        replacement: String,
    },
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    FoldSelection,
    ToggleWrap,
    ToggleReadOnly,
}

#[derive(Clone, Copy)]
pub(crate) struct TextContextMenuState {
    pub undo: MenuActionState,
    pub redo: MenuActionState,
    pub cut: MenuActionState,
    pub copy: MenuActionState,
    pub paste: MenuActionState,
    pub select_all: MenuActionState,
    pub fold_selection: MenuActionState,
    pub toggle_wrap: MenuActionState,
    pub toggle_read_only: MenuActionState,
}

pub(crate) struct ContextMenuBuilder {
    action_group_name: String,
    menu: gio::Menu,
    section: gio::Menu,
    section_has_items: bool,
}

impl<A> ActionMenuItem<A> {
    pub(crate) fn new(label: impl Into<String>, action: A, enabled: bool) -> Self {
        Self {
            label: label.into(),
            action,
            enabled,
        }
    }
}

impl<A> ActionMenuSection<A> {
    pub(crate) fn new(items: Vec<ActionMenuItem<A>>) -> Self {
        Self { items }
    }
}

impl MenuActionState {
    pub(crate) fn visible(enabled: bool) -> Self {
        Self {
            visible: true,
            enabled,
        }
    }
}

pub(crate) fn builder(action_group_name: &str) -> ContextMenuBuilder {
    ContextMenuBuilder::new(action_group_name)
}

impl ContextMenuBuilder {
    pub(crate) fn new(action_group_name: &str) -> Self {
        Self {
            action_group_name: action_group_name.to_string(),
            menu: gio::Menu::new(),
            section: gio::Menu::new(),
            section_has_items: false,
        }
    }

    pub(crate) fn item(self, label: &str, action: &str) -> Self {
        self.menu_item(label, action, None, None)
    }

    pub(crate) fn shortcut_item(self, label: &str, action: &str, shortcut: &str) -> Self {
        self.menu_item(label, action, None, Some(shortcut))
    }

    pub(crate) fn target_item(self, label: &str, action: &str, target: &str) -> Self {
        self.menu_item(label, action, Some(target), None)
    }

    pub(crate) fn separator(mut self) -> Self {
        self.flush_section();
        self
    }

    pub(crate) fn build(mut self) -> gio::Menu {
        self.flush_section();
        self.menu
    }

    pub(crate) fn popup<W>(
        self,
        parent: &W,
        x: f64,
        y: f64,
        actions: &gio::SimpleActionGroup,
        active_context_menu: &RefCell<Option<gtk::Popover>>,
    ) -> gtk::PopoverMenu
    where
        W: IsA<gtk::Widget>,
    {
        let action_group_name = self.action_group_name.clone();
        let menu = self.build();
        popup_model_menu(
            parent,
            x,
            y,
            &menu,
            &action_group_name,
            actions,
            active_context_menu,
        )
    }

    fn menu_item(
        mut self,
        label: &str,
        action: &str,
        target: Option<&str>,
        shortcut: Option<&str>,
    ) -> Self {
        let detailed_action = format!("{}.{}", self.action_group_name, action);
        let item = match target {
            Some(target) => string_target_item(label, &detailed_action, target),
            None => gio::MenuItem::new(Some(label), Some(&detailed_action)),
        };
        if let Some(shortcut) = shortcut {
            item.set_attribute_value("accel", Some(&shortcut.to_variant()));
        }
        self.section.append_item(&item);
        self.section_has_items = true;
        self
    }

    fn flush_section(&mut self) {
        if !self.section_has_items {
            return;
        }
        let section = std::mem::replace(&mut self.section, gio::Menu::new());
        self.menu.append_section(None, &section);
        self.section_has_items = false;
    }
}

pub(crate) fn popup_action_menu<W, A, F>(
    parent: &W,
    x: f64,
    y: f64,
    sections: Vec<ActionMenuSection<A>>,
    activate: F,
) -> gtk::PopoverMenu
where
    W: IsA<gtk::Widget>,
    A: Clone + 'static,
    F: Fn(A) + 'static,
{
    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();
    let activate = Rc::new(activate);
    let mut index = 0usize;

    for section in sections {
        let section_menu = gio::Menu::new();
        let mut has_items = false;

        for item in section.items {
            let name = format!("item-{index}");
            let detailed_action = format!("context.{name}");
            section_menu.append(Some(&item.label), Some(&detailed_action));

            let action = gio::SimpleAction::new(&name, None);
            action.set_enabled(item.enabled);
            action.connect_activate({
                let activate = activate.clone();
                let item_action = item.action.clone();

                move |_, _| activate(item_action.clone())
            });
            actions.add_action(&action);
            has_items = true;
            index += 1;
        }

        if has_items {
            menu.append_section(None, &section_menu);
        }
    }

    let popover = popover_menu_for_model(parent, &menu, x, y);
    popover.insert_action_group("context", Some(&actions));
    popover.popup();
    popover
}

pub(crate) fn popup_model_menu<W>(
    parent: &W,
    x: f64,
    y: f64,
    menu: &gio::Menu,
    action_group_name: &str,
    actions: &gio::SimpleActionGroup,
    active_context_menu: &RefCell<Option<gtk::Popover>>,
) -> gtk::PopoverMenu
where
    W: IsA<gtk::Widget>,
{
    log::debug!("showing {action_group_name} context menu");
    let popover = popover_menu_for_model(parent, menu, x, y);
    popover.insert_action_group(action_group_name, Some(actions));
    retain_context_menu(active_context_menu, popover.upcast_ref::<gtk::Popover>());
    popover.popup();
    popover
}

pub(crate) fn text_context_menu_sections(
    state: TextContextMenuState,
) -> Vec<ActionMenuSection<TextContextAction>> {
    let groups = [
        vec![
            text_item("Undo", TextContextAction::Undo, state.undo),
            text_item("Redo", TextContextAction::Redo, state.redo),
        ],
        vec![
            text_item("Cut", TextContextAction::Cut, state.cut),
            text_item("Copy", TextContextAction::Copy, state.copy),
            text_item("Paste", TextContextAction::Paste, state.paste),
            text_item("Select All", TextContextAction::SelectAll, state.select_all),
        ],
        vec![
            text_item(
                "Fold Selection",
                TextContextAction::FoldSelection,
                state.fold_selection,
            ),
            text_item(
                "Toggle Wrap",
                TextContextAction::ToggleWrap,
                state.toggle_wrap,
            ),
            text_item(
                "Toggle Read Only",
                TextContextAction::ToggleReadOnly,
                state.toggle_read_only,
            ),
        ],
    ];

    groups
        .into_iter()
        .filter_map(|items| {
            let items = items.into_iter().flatten().collect::<Vec<_>>();
            (!items.is_empty()).then(|| ActionMenuSection::new(items))
        })
        .collect()
}

pub(crate) fn install_terminal_context_menu(terminal: &vte4::Terminal) {
    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let terminal = terminal.clone();

        move |gesture, _, x, y| {
            if gesture.current_button() != 3 {
                return;
            }

            show_terminal_context_menu(&terminal, x, y);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    terminal.add_controller(click);
}

pub(crate) fn copy_terminal_selection(terminal: &vte4::Terminal) {
    let Some(text) = terminal.text_selected(vte4::Format::Text) else {
        return;
    };
    if text.is_empty() {
        return;
    }

    terminal.clipboard().set_text(&text);
}

pub(crate) fn copy_terminal_all(terminal: &vte4::Terminal) {
    let (_cursor_column, cursor_row) = terminal.cursor_position();
    let visible_end_row = terminal.row_count().saturating_sub(1);
    let end_row = cursor_row.max(visible_end_row);
    let end_col = terminal.column_count().max(1);
    let (text, _) = terminal.text_range_format(vte4::Format::Text, 0, 0, end_row, end_col);
    let Some(text) = text else {
        log::debug!("terminal copy-all skipped reason=no-text");
        return;
    };
    if text.is_empty() {
        log::debug!("terminal copy-all skipped reason=empty");
        return;
    }

    terminal.clipboard().set_text(&text);
    log::info!(
        "terminal copy-all copied rows=0..{} chars={}",
        end_row,
        text.chars().count()
    );
}

pub(crate) fn copy_terminal_screen(terminal: &vte4::Terminal) {
    let Some(scroller) = terminal
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
    else {
        log::warn!("terminal copy-screen failed reason=no-scrolled-window");
        return;
    };

    let adjustment = scroller.vadjustment();
    let char_height = terminal.char_height().max(1) as f64;
    let start_row = (adjustment.value() / char_height).floor().max(0.0) as libc::c_long;
    let visible_rows = (adjustment.page_size() / char_height).ceil().max(1.0) as libc::c_long;
    let end_row = start_row + visible_rows;
    let end_col = terminal.column_count().max(1);
    let (text, _) = terminal.text_range_format(vte4::Format::Text, start_row, 0, end_row, end_col);
    let Some(text) = text else {
        log::debug!("terminal copy-screen skipped reason=no-text");
        return;
    };
    if text.is_empty() {
        log::debug!("terminal copy-screen skipped reason=empty");
        return;
    }

    terminal.clipboard().set_text(&text);
    log::info!(
        "terminal copy-screen copied rows={}..{} chars={}",
        start_row,
        end_row,
        text.chars().count()
    );
}

pub(crate) fn popover_menu_for_model<W>(
    parent: &W,
    menu: &gio::Menu,
    x: f64,
    y: f64,
) -> gtk::PopoverMenu
where
    W: IsA<gtk::Widget>,
{
    let (parent, x, y) = normalized_popup_anchor(parent, x, y);
    let popover = gtk::PopoverMenu::from_model(None::<&gio::Menu>);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_menu_model(Some(menu));
    popover.set_parent(&parent);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover
}

fn normalized_popup_anchor<W>(parent: &W, x: f64, y: f64) -> (gtk::Widget, f64, f64)
where
    W: IsA<gtk::Widget>,
{
    let widget = parent.as_ref();
    let mut ancestor = widget.clone();
    while let Some(next) = ancestor.parent() {
        if ancestor.is::<gtk::ScrolledWindow>() {
            if let Some((anchor_x, anchor_y)) = widget.translate_coordinates(&next, x, y) {
                log::debug!("context menu anchor moved outside scrolled ancestor");
                return (next, anchor_x, anchor_y);
            }
            break;
        }
        ancestor = next;
    }

    (widget.clone(), x, y)
}

pub(crate) fn retain_context_menu(
    active_context_menu: &RefCell<Option<gtk::Popover>>,
    popover: &gtk::Popover,
) {
    if let Some(existing) = active_context_menu.borrow_mut().replace(popover.clone()) {
        existing.popdown();
    }
}

pub(crate) fn track_context_menu_event_time(popover: &gtk::PopoverMenu, event_time: Rc<Cell<u32>>) {
    let click = gtk::GestureClick::builder().button(0).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |gesture, _, _, _| {
        event_time.set(gesture.current_event_time());
    });
    popover.add_controller(click);
}

pub(crate) fn add_string_menu_action<F>(
    group: &gio::SimpleActionGroup,
    name: &str,
    activate: F,
) -> gio::SimpleAction
where
    F: Fn(&str) + 'static,
{
    let action = gio::SimpleAction::new(
        name,
        Some(gtk::glib::VariantTy::new("s").expect("valid string variant type")),
    );
    action.connect_activate(move |_, parameter| {
        if let Some(value) = parameter.and_then(|parameter| parameter.str()) {
            activate(value);
        };
    });
    group.add_action(&action);
    action
}

pub(crate) fn string_target_item(label: &str, action: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    item
}

fn show_terminal_context_menu(terminal: &vte4::Terminal, x: f64, y: f64) {
    #[derive(Clone, Copy)]
    enum TerminalAction {
        Copy,
        Paste,
        SelectAll,
        CopyAll,
        CopyScreen,
    }

    popup_action_menu(
        terminal,
        x,
        y,
        vec![
            ActionMenuSection::new(vec![
                ActionMenuItem::new("Copy", TerminalAction::Copy, terminal.has_selection()),
                ActionMenuItem::new("Paste", TerminalAction::Paste, true),
                ActionMenuItem::new("Select All", TerminalAction::SelectAll, true),
            ]),
            ActionMenuSection::new(vec![
                ActionMenuItem::new("Copy All", TerminalAction::CopyAll, true),
                ActionMenuItem::new("Copy Screen", TerminalAction::CopyScreen, true),
            ]),
        ],
        {
            let terminal = terminal.clone();

            move |action| {
                match action {
                    TerminalAction::Copy => copy_terminal_selection(&terminal),
                    TerminalAction::Paste => terminal.paste_clipboard(),
                    TerminalAction::SelectAll => terminal.select_all(),
                    TerminalAction::CopyAll => copy_terminal_all(&terminal),
                    TerminalAction::CopyScreen => copy_terminal_screen(&terminal),
                }
                terminal.grab_focus();
            }
        },
    );
}

fn text_item(
    label: &'static str,
    action: TextContextAction,
    state: MenuActionState,
) -> Option<ActionMenuItem<TextContextAction>> {
    state
        .visible
        .then(|| ActionMenuItem::new(label, action, state.enabled))
}
