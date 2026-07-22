use adw::prelude::*;
use gtk::{gdk, gio};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct ActionMenuItem<A> {
    pub label: String,
    pub icon_name: Option<String>,
    pub action: A,
    pub enabled: bool,
}

#[derive(Clone)]
pub struct ActionMenuSection<A> {
    pub items: Vec<ActionMenuItem<A>>,
}

#[derive(Clone, Copy)]
pub struct MenuActionState {
    pub visible: bool,
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub enum TextContextAction {
    ApplyMarkdownFix {
        edits: Vec<crate::markdown_lint::MarkdownLintEdit>,
    },
    AddMarkdownLintIgnore {
        rule_name: String,
    },
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
pub struct TextContextMenuState {
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

pub struct ContextMenuBuilder {
    action_group_name: String,
    menu: gio::Menu,
    section: gio::Menu,
    section_has_items: bool,
}

impl<A> ActionMenuItem<A> {
    pub fn new(label: impl Into<String>, action: A, enabled: bool) -> Self {
        Self {
            label: label.into(),
            icon_name: None,
            action,
            enabled,
        }
    }

    pub fn with_icon(
        label: impl Into<String>,
        icon_name: impl Into<String>,
        action: A,
        enabled: bool,
    ) -> Self {
        Self {
            label: label.into(),
            icon_name: Some(icon_name.into()),
            action,
            enabled,
        }
    }
}

impl<A> ActionMenuSection<A> {
    pub fn new(items: Vec<ActionMenuItem<A>>) -> Self {
        Self { items }
    }
}

impl MenuActionState {
    pub fn visible(enabled: bool) -> Self {
        Self {
            visible: true,
            enabled,
        }
    }
}

pub fn builder(action_group_name: &str) -> ContextMenuBuilder {
    ContextMenuBuilder::new(action_group_name)
}

impl ContextMenuBuilder {
    pub fn new(action_group_name: &str) -> Self {
        Self {
            action_group_name: action_group_name.to_string(),
            menu: gio::Menu::new(),
            section: gio::Menu::new(),
            section_has_items: false,
        }
    }

    pub fn item(self, label: &str, action: &str) -> Self {
        self.menu_item(label, action, None, None)
    }

    pub fn shortcut_item(self, label: &str, action: &str, shortcut: &str) -> Self {
        self.menu_item(label, action, None, Some(shortcut))
    }

    pub fn target_item(self, label: &str, action: &str, target: &str) -> Self {
        self.menu_item(label, action, Some(target), None)
    }

    pub fn submenu(mut self, label: &str, submenu: &impl IsA<gio::MenuModel>) -> Self {
        self.section.append_submenu(Some(label), submenu);
        self.section_has_items = true;
        self
    }

    pub fn separator(mut self) -> Self {
        self.flush_section();
        self
    }

    pub fn build(mut self) -> gio::Menu {
        self.flush_section();
        self.menu
    }

    pub fn popup<W>(
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

pub fn popup_action_menu<W, A, F>(
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
            let menu_item = gio::MenuItem::new(Some(&item.label), Some(&detailed_action));
            if let Some(icon_name) = &item.icon_name {
                menu_item.set_icon(&gio::ThemedIcon::new(icon_name));
            }
            section_menu.append_item(&menu_item);

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

pub fn popup_model_menu<W>(
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

pub fn text_context_menu_sections(
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

pub fn popover_menu_for_model<W>(parent: &W, menu: &gio::Menu, x: f64, y: f64) -> gtk::PopoverMenu
where
    W: IsA<gtk::Widget>,
{
    let (parent, x, y) = normalized_popup_anchor(parent, x, y);
    let popover = gtk::PopoverMenu::from_model(Some(menu));
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
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

pub fn retain_context_menu(
    active_context_menu: &RefCell<Option<gtk::Popover>>,
    popover: &gtk::Popover,
) {
    if let Some(existing) = active_context_menu.borrow_mut().replace(popover.clone()) {
        existing.popdown();
    }
}

pub fn add_string_menu_action<F>(
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

pub fn string_target_item(label: &str, action: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    item
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
