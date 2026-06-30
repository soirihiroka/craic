use adw::prelude::*;
use craic_diff_ui::{Element, PartialEqRenderState};
use gtk::gdk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type QueryCallback = Rc<dyn Fn(String)>;
type BoolCallback = Rc<dyn Fn(bool)>;
type TagCallback = Rc<dyn Fn(String, bool)>;
type UnitCallback = Rc<dyn Fn()>;

pub(crate) const SEARCH_PANEL_CSS: &str = r#"
box.search-panel {
    background-color: @headerbar_bg_color;
}
button.search-option-toggle {
    min-width: 32px;
    min-height: 32px;
    padding: 0 4px;
}
button.search-tag-pill {
    min-height: 0;
    padding: 5px 8px 5px 11px;
}
label.search-tag-count {
    min-width: 14px;
    padding: 1px 5px;
    border-radius: 9999px;
    background-color: alpha(@view_fg_color, 0.16);
    font-weight: 700;
}
box.search-tags-fade {
    min-width: 18px;
}
box.search-tags-fade-left {
    background-image: linear-gradient(to right, @headerbar_bg_color, alpha(@headerbar_bg_color, 0));
}
box.search-tags-fade-right {
    background-image: linear-gradient(to left, @headerbar_bg_color, alpha(@headerbar_bg_color, 0));
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchTag {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) count: Option<usize>,
    pub(crate) active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchTagRenderState {
    tag: SearchTag,
    first: bool,
    last: bool,
}

#[derive(Clone)]
pub(crate) struct SearchPanel {
    root: gtk::Revealer,
    search_bar: gtk::SearchBar,
    entry: gtk::SearchEntry,
    status: gtk::Label,
    options: gtk::Box,
    case_button: gtk::ToggleButton,
    word_button: gtk::ToggleButton,
    regex_button: gtk::ToggleButton,
    previous_button: gtk::Button,
    next_button: gtk::Button,
    tags_box: gtk::Box,
    tags_overlay: gtk::Overlay,
    tags_scroller: gtk::ScrolledWindow,
    tags_fade_left: gtk::Box,
    tags_fade_right: gtk::Box,
    empty_bottom_spacer: gtk::Box,
    tag_reconciler: Rc<RefCell<craic_diff_ui::gtk::BoxReconciler<String, SearchTagRenderState>>>,
    clear_on_close: Rc<Cell<bool>>,
    query_callbacks: Rc<RefCell<Vec<QueryCallback>>>,
    open_callbacks: Rc<RefCell<Vec<UnitCallback>>>,
    close_callbacks: Rc<RefCell<Vec<UnitCallback>>>,
    case_callbacks: Rc<RefCell<Vec<BoolCallback>>>,
    word_callbacks: Rc<RefCell<Vec<BoolCallback>>>,
    regex_callbacks: Rc<RefCell<Vec<BoolCallback>>>,
    tag_callbacks: Rc<RefCell<Vec<TagCallback>>>,
    previous_callbacks: Rc<RefCell<Vec<UnitCallback>>>,
    next_callbacks: Rc<RefCell<Vec<UnitCallback>>>,
}

impl SearchPanel {
    pub(crate) fn new(placeholder: &str) -> Self {
        let root = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .hexpand(true)
            .build();
        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        panel.add_css_class("search-panel");
        let search_bar = gtk::SearchBar::builder()
            .hexpand(true)
            .visible(false)
            .build();

        let entry = gtk::SearchEntry::builder()
            .placeholder_text(placeholder)
            .search_delay(150)
            .hexpand(true)
            .build();
        search_bar.connect_entry(&entry);

        let status = gtk::Label::builder()
            .width_chars(9)
            .xalign(0.5)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        status.add_css_class("dim-label");

        let case_button = option_button("Aa", "Match case");
        let word_button = option_button("ab", "Match whole word");
        let regex_button = option_button(".*", "Use regular expression");
        let options = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        options.add_css_class("linked");
        options.append(&case_button);
        options.append(&word_button);
        options.append(&regex_button);

        let previous_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Previous match")
            .build();
        previous_button.add_css_class("flat");
        let next_button = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Next match")
            .build();
        next_button.add_css_class("flat");
        let tags_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Start)
            .build();
        let tags_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .height_request(42)
            .child(&tags_box)
            .build();
        tags_scroller.set_min_content_width(0);
        tags_scroller.set_propagate_natural_width(false);
        tags_scroller.set_overlay_scrolling(true);
        let tags_overlay = gtk::Overlay::builder()
            .hexpand(true)
            .height_request(42)
            .visible(false)
            .build();
        tags_overlay.set_child(Some(&tags_scroller));
        let tags_fade_left = tag_fade("search-tags-fade-left", gtk::Align::Start);
        let tags_fade_right = tag_fade("search-tags-fade-right", gtk::Align::End);
        tags_overlay.add_overlay(&tags_fade_left);
        tags_overlay.set_measure_overlay(&tags_fade_left, false);
        tags_overlay.add_overlay(&tags_fade_right);
        tags_overlay.set_measure_overlay(&tags_fade_right, false);

        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_start(8)
            .margin_end(6)
            .build();
        controls.append(&entry);
        controls.append(&status);
        controls.append(&options);
        controls.append(&previous_button);
        controls.append(&next_button);

        let empty_bottom_spacer = gtk::Box::builder().height_request(6).visible(false).build();

        panel.append(&controls);
        panel.append(&tags_overlay);
        panel.append(&empty_bottom_spacer);
        panel.append(&search_bar);
        root.set_child(Some(&panel));

        let panel = Self {
            root,
            search_bar,
            entry,
            status,
            options,
            case_button,
            word_button,
            regex_button,
            previous_button,
            next_button,
            tags_box,
            tags_overlay,
            tags_scroller,
            tags_fade_left,
            tags_fade_right,
            empty_bottom_spacer,
            tag_reconciler: Rc::new(RefCell::new(craic_diff_ui::gtk::BoxReconciler::new())),
            clear_on_close: Rc::new(Cell::new(true)),
            query_callbacks: Rc::new(RefCell::new(Vec::new())),
            open_callbacks: Rc::new(RefCell::new(Vec::new())),
            close_callbacks: Rc::new(RefCell::new(Vec::new())),
            case_callbacks: Rc::new(RefCell::new(Vec::new())),
            word_callbacks: Rc::new(RefCell::new(Vec::new())),
            regex_callbacks: Rc::new(RefCell::new(Vec::new())),
            tag_callbacks: Rc::new(RefCell::new(Vec::new())),
            previous_callbacks: Rc::new(RefCell::new(Vec::new())),
            next_callbacks: Rc::new(RefCell::new(Vec::new())),
        };
        panel.connect_internal();
        panel.connect_tag_scroll();
        panel.connect_tag_fades();
        panel
    }

    pub(crate) fn widget(&self) -> gtk::Revealer {
        self.root.clone()
    }

    pub(crate) fn set_key_capture_widget<W: IsA<gtk::Widget>>(&self, widget: &W) {
        self.search_bar.set_key_capture_widget(Some(widget));
    }

    pub(crate) fn install_shortcuts<W: IsA<gtk::Widget>>(&self, widget: &W) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let panel = self.clone();

            move |_, key, _, modifiers| {
                let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                if ctrl && !alt && matches!(key, gdk::Key::f | gdk::Key::F) {
                    panel.toggle();
                    return gtk::glib::Propagation::Stop;
                }
                if panel.search_bar.is_search_mode() && key == gdk::Key::Escape {
                    panel.close();
                    return gtk::glib::Propagation::Stop;
                }
                if panel.entry.has_focus() && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
                    if shift {
                        panel.emit_previous();
                    } else {
                        panel.emit_next();
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            }
        });
        widget.add_controller(keys);
    }

    pub(crate) fn set_clear_on_close(&self, clear: bool) {
        self.clear_on_close.set(clear);
    }

    pub(crate) fn set_options_visible(&self, visible: bool) {
        self.options.set_visible(visible);
        self.case_button.set_visible(visible);
        self.word_button.set_visible(visible);
        self.regex_button.set_visible(visible);
    }

    pub(crate) fn set_navigation_visible(&self, visible: bool) {
        self.previous_button.set_visible(visible);
        self.next_button.set_visible(visible);
    }

    pub(crate) fn open(&self) {
        self.search_bar.set_search_mode(true);
        self.sync_tag_visibility();
        self.root.set_reveal_child(true);
        self.entry.grab_focus();
        for callback in self.open_callbacks.borrow().iter() {
            callback();
        }
        log::debug!("search panel opened query_len={}", self.entry.text().len());
    }

    pub(crate) fn toggle(&self) {
        if self.search_bar.is_search_mode() {
            self.close();
        } else {
            self.open();
        }
        log::debug!(
            "search panel toggled open={}",
            self.search_bar.is_search_mode()
        );
    }

    pub(crate) fn close(&self) {
        if self.clear_on_close.get() {
            self.clear();
        }
        self.search_bar.set_search_mode(false);
        self.root.set_reveal_child(false);
        for callback in self.close_callbacks.borrow().iter() {
            callback();
        }
        log::debug!("search panel closed");
    }

    pub(crate) fn clear(&self) {
        if !self.entry.text().is_empty() {
            self.entry.set_text("");
        }
        log::debug!("search panel cleared");
    }

    pub(crate) fn query(&self) -> String {
        self.entry.text().to_string()
    }

    pub(crate) fn set_query(&self, query: &str, select: bool) {
        self.entry.set_text(query);
        if select {
            self.entry.select_region(0, -1);
        }
    }

    pub(crate) fn set_status(&self, status: &str) {
        self.status.set_label(status);
        self.status.set_visible(!status.is_empty());
    }

    pub(crate) fn has_focus(&self) -> bool {
        widget_tree_has_focus(&self.root.clone().upcast::<gtk::Widget>())
            || widget_tree_has_focus(&self.entry.clone().upcast::<gtk::Widget>())
    }

    pub(crate) fn set_tags(&self, tags: Vec<SearchTag>) {
        let has_tags = !tags.is_empty();
        let last_index = tags.len().saturating_sub(1);
        let elements = tags
            .into_iter()
            .enumerate()
            .map(|(index, tag)| {
                Element::new(
                    tag.id.clone(),
                    SearchTagRenderState {
                        tag,
                        first: index == 0,
                        last: index == last_index,
                    },
                )
            })
            .collect::<Vec<_>>();
        let stats = self.tag_reconciler.borrow_mut().reconcile(
            &self.tags_box,
            elements,
            PartialEqRenderState,
            {
                let panel = self.clone();

                move |_, _, state| panel.tag_button(state.clone()).upcast::<gtk::Widget>()
            },
            move |_, widget, _, next| update_tag_button(widget, next),
        );
        if stats.changed() {
            log::debug!(
                "search panel tags reconciled inserted={} updated={} moved={} removed={} unchanged={}",
                stats.inserted,
                stats.updated,
                stats.moved,
                stats.removed,
                stats.unchanged
            );
        }
        if self.search_bar.is_search_mode() {
            self.tags_overlay.set_visible(has_tags);
            self.empty_bottom_spacer.set_visible(!has_tags);
        }
        self.update_tag_fades();
    }

    pub(crate) fn connect_query_changed<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        self.query_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(crate) fn connect_opened<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.open_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(crate) fn connect_closed<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.close_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(crate) fn connect_option_toggled<F>(&self, option: SearchOption, callback: F)
    where
        F: Fn(bool) + 'static,
    {
        match option {
            SearchOption::CaseSensitive => self.case_callbacks.borrow_mut().push(Rc::new(callback)),
            SearchOption::WholeWord => self.word_callbacks.borrow_mut().push(Rc::new(callback)),
            SearchOption::Regex => self.regex_callbacks.borrow_mut().push(Rc::new(callback)),
        }
    }

    pub(crate) fn connect_tag_toggled<F>(&self, callback: F)
    where
        F: Fn(String, bool) + 'static,
    {
        self.tag_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(crate) fn connect_previous<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.previous_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(crate) fn connect_next<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.next_callbacks.borrow_mut().push(Rc::new(callback));
    }

    fn connect_internal(&self) {
        self.entry.connect_search_changed({
            let callbacks = self.query_callbacks.clone();

            move |entry| {
                let query = entry.text().to_string();
                for callback in callbacks.borrow().iter() {
                    callback(query.clone());
                }
                log::debug!("search panel query updated len={}", query.len());
            }
        });
        self.entry.connect_stop_search({
            let panel = self.clone();

            move |_| panel.close()
        });
        self.case_button.connect_toggled({
            let callbacks = self.case_callbacks.clone();

            move |button| emit_option(&callbacks, SearchOption::CaseSensitive, button.is_active())
        });
        self.word_button.connect_toggled({
            let callbacks = self.word_callbacks.clone();

            move |button| emit_option(&callbacks, SearchOption::WholeWord, button.is_active())
        });
        self.regex_button.connect_toggled({
            let callbacks = self.regex_callbacks.clone();

            move |button| emit_option(&callbacks, SearchOption::Regex, button.is_active())
        });
        self.previous_button.connect_clicked({
            let panel = self.clone();

            move |_| panel.emit_previous()
        });
        self.next_button.connect_clicked({
            let panel = self.clone();

            move |_| panel.emit_next()
        });
    }

    fn connect_tag_scroll(&self) {
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL
                | gtk::EventControllerScrollFlags::HORIZONTAL
                | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll({
            let tags_scroller = self.tags_scroller.clone();

            move |_, dx, dy| {
                let adjustment = tags_scroller.hadjustment();
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                if delta == 0.0 {
                    return gtk::glib::Propagation::Proceed;
                }

                let step = if adjustment.step_increment() > 0.0 {
                    adjustment.step_increment()
                } else {
                    48.0
                };
                let lower = adjustment.lower();
                let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
                let next = (adjustment.value() + delta * step).clamp(lower, upper);
                if (next - adjustment.value()).abs() < f64::EPSILON {
                    return gtk::glib::Propagation::Proceed;
                }

                adjustment.set_value(next);
                gtk::glib::Propagation::Stop
            }
        });
        self.tags_box.add_controller(scroll);
    }

    fn connect_tag_fades(&self) {
        self.tags_scroller.hadjustment().connect_value_changed({
            let panel = self.clone();

            move |_| panel.update_tag_fades()
        });
        self.tags_scroller.hadjustment().connect_changed({
            let panel = self.clone();

            move |_| panel.update_tag_fades()
        });
    }

    fn emit_previous(&self) {
        for callback in self.previous_callbacks.borrow().iter() {
            callback();
        }
    }

    fn emit_next(&self) {
        for callback in self.next_callbacks.borrow().iter() {
            callback();
        }
    }

    fn sync_tag_visibility(&self) {
        let has_tags = self.tags_box.first_child().is_some();
        self.tags_overlay.set_visible(has_tags);
        self.empty_bottom_spacer.set_visible(!has_tags);
        self.update_tag_fades();
    }

    fn update_tag_fades(&self) {
        let adjustment = self.tags_scroller.hadjustment();
        let lower = adjustment.lower();
        let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
        let value = adjustment.value().clamp(lower, upper);
        let can_scroll = upper > lower + f64::EPSILON;
        self.tags_fade_left
            .set_visible(can_scroll && value > lower + 0.5);
        self.tags_fade_right
            .set_visible(can_scroll && value < upper - 0.5);
    }

    fn tag_button(&self, state: SearchTagRenderState) -> gtk::ToggleButton {
        let button = gtk::ToggleButton::builder()
            .child(&tag_content(&state.tag))
            .active(state.tag.active)
            .valign(gtk::Align::Start)
            .margin_top(6)
            .margin_bottom(12)
            .build();
        apply_tag_edge_margins(&button, state.first, state.last);
        button.add_css_class("pill");
        button.add_css_class("search-tag-pill");
        button.connect_toggled({
            let callbacks = self.tag_callbacks.clone();
            let id = state.tag.id;

            move |button| {
                let active = button.is_active();
                for callback in callbacks.borrow().iter() {
                    callback(id.clone(), active);
                }
                log::debug!("search panel tag toggled tag={} active={}", id, active);
            }
        });
        button
    }
}

fn option_button(label: &str, tooltip: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .label(label)
        .tooltip_text(tooltip)
        .width_request(32)
        .height_request(34)
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("search-option-toggle");
    button
}

fn tag_fade(class_name: &str, halign: gtk::Align) -> gtk::Box {
    let fade = gtk::Box::builder()
        .halign(halign)
        .valign(gtk::Align::Fill)
        .width_request(18)
        .vexpand(true)
        .visible(false)
        .build();
    fade.add_css_class("search-tags-fade");
    fade.add_css_class(class_name);
    fade.set_can_target(false);
    fade
}

fn emit_option(callbacks: &Rc<RefCell<Vec<BoolCallback>>>, option: SearchOption, active: bool) {
    for callback in callbacks.borrow().iter() {
        callback(active);
    }
    log::debug!("search panel option toggled option={option:?} active={active}");
}

fn tag_content(tag: &SearchTag) -> gtk::Box {
    let label = gtk::Label::new(Some(&tag.label));
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    content.append(&label);
    if let Some(count) = tag.count {
        let count_label = gtk::Label::new(Some(&count.to_string()));
        count_label.add_css_class("numeric");
        count_label.add_css_class("search-tag-count");
        content.append(&count_label);
    }
    content
}

fn update_tag_button(widget: &gtk::Widget, state: &SearchTagRenderState) {
    let Ok(button) = widget.clone().downcast::<gtk::ToggleButton>() else {
        return;
    };
    if button.is_active() != state.tag.active {
        button.set_active(state.tag.active);
    }
    button.set_child(Some(&tag_content(&state.tag)));
    apply_tag_edge_margins(&button, state.first, state.last);
}

fn apply_tag_edge_margins(button: &gtk::ToggleButton, first: bool, last: bool) {
    button.set_margin_start(if first { 8 } else { 0 });
    button.set_margin_end(if last { 8 } else { 0 });
}

fn widget_tree_has_focus(widget: &gtk::Widget) -> bool {
    if widget.has_focus() {
        return true;
    }

    let mut child = widget.first_child();
    while let Some(child_widget) = child {
        let next = child_widget.next_sibling();
        if widget_tree_has_focus(&child_widget) {
            return true;
        }
        child = next;
    }

    false
}
