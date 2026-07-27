use super::menus::{add_context_menu, thread_command_menu};
use super::tools::codex_tools_menu;
use super::transcript::{pending_request_row, timeline_row};
use super::{
    ChatConnectionStatus, ChatSelector, CodexChatAction, CollaborationParticipantStatus,
    CollaborationProgress, ComposerAttachment, ComposerAttachmentKind, ComposerSubmission,
    PendingRequest, PlanProgress, PlanStepStatus, QueueDirection, QueuedSubmission, SelectorOption,
    TimelineItem, TokenUsage,
};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::config;

const TRANSCRIPT_FOLLOW_DISTANCE: f64 = 72.0;
thread_local! {
    static CHAT_FONT_PROVIDER: gtk::CssProvider = {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
            );
        }
        provider
    };
}
const CHAT_SELECTORS: [ChatSelector; 8] = [
    ChatSelector::Model,
    ChatSelector::Reasoning,
    ChatSelector::ReasoningSummary,
    ChatSelector::Personality,
    ChatSelector::Permissions,
    ChatSelector::Collaboration,
    ChatSelector::ServiceTier,
    ChatSelector::ApprovalReviewer,
];

pub(super) type ActionCallback = Rc<dyn Fn(CodexChatAction)>;

#[derive(Clone, Debug)]
enum TranscriptEntry {
    Timeline(TimelineItem),
    Pending(PendingRequest),
}

struct SelectorControl {
    dropdown: gtk::DropDown,
    ids: RefCell<Vec<String>>,
    updating: Cell<bool>,
}

struct CodexChatViewState {
    model: gio::ListStore,
    timeline_indices: RefCell<HashMap<String, u32>>,
    pending_indices: RefCell<HashMap<String, u32>>,
    queued_timeline_updates: RefCell<Vec<TimelineItem>>,
    timeline_flush_scheduled: Cell<bool>,
    transcript_stack: gtk::Stack,
    transcript_scroller: gtk::ScrolledWindow,
    status_icon: gtk::Image,
    status_label: gtk::Label,
    usage_label: gtk::Label,
    usage_icon: gtk::Image,
    usage_progress: gtk::ProgressBar,
    older_turns_row: gtk::Box,
    older_turns_button: gtk::Button,
    older_turns_spinner: adw::Spinner,
    older_turns_available: Cell<bool>,
    header_action_widgets: Vec<gtk::Widget>,
    selectors: HashMap<ChatSelector, SelectorControl>,
    progress_revealer: gtk::Revealer,
    plan_progress_box: gtk::Box,
    collaboration_progress_box: gtk::Box,
    composer: gtk::TextView,
    composer_placeholder: gtk::Label,
    queued_submissions_box: gtk::Box,
    queued_submissions_list: gtk::ListBox,
    attachment_flow: gtk::FlowBox,
    attachments: RefCell<Vec<ComposerAttachment>>,
    send_button: gtk::Button,
    steer_button: gtk::Button,
    interrupt_button: gtk::Button,
    callbacks: Rc<RefCell<Vec<ActionCallback>>>,
    connected: Cell<bool>,
    turn_active: Cell<bool>,
    composer_allowed: Cell<bool>,
}

#[derive(Clone)]
pub struct CodexChatView {
    pub root: gtk::Box,
    state: Rc<CodexChatViewState>,
}

impl CodexChatView {
    pub fn new() -> Self {
        let callbacks = Rc::new(RefCell::new(Vec::<ActionCallback>::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let factory = transcript_factory(callbacks.clone());
        let transcript = gtk::ListView::new(Some(selection), Some(factory));
        transcript.set_hexpand(true);
        transcript.set_vexpand(true);
        transcript.set_single_click_activate(false);

        let transcript_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&transcript)
            .build();
        let empty_label = gtk::Label::builder()
            .label("Start a Codex conversation")
            .css_classes(["title-2", "dim-label"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        let transcript_stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        transcript_stack.add_named(&empty_label, Some("empty"));
        transcript_stack.add_named(&transcript_scroller, Some("transcript"));
        transcript_stack.set_visible_child_name("empty");

        let older_turns_button = gtk::Button::with_label("Load older messages");
        older_turns_button.add_css_class("flat");
        older_turns_button.connect_clicked({
            let callbacks = callbacks.clone();
            move |_| emit_action(&callbacks, CodexChatAction::LoadOlderTurns)
        });
        let older_turns_spinner = adw::Spinner::new();
        older_turns_spinner.set_size_request(16, 16);
        older_turns_spinner.set_visible(false);
        let older_turns_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::Center)
            .margin_top(2)
            .margin_bottom(2)
            .visible(false)
            .build();
        older_turns_row.append(&older_turns_spinner);
        older_turns_row.append(&older_turns_button);
        let transcript_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        transcript_panel.append(&older_turns_row);
        transcript_panel.append(&transcript_stack);

        let status_label = gtk::Label::builder()
            .label("Disconnected")
            .css_classes(["dim-label"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .build();
        let status_icon = gtk::Image::from_icon_name("network-offline-symbolic");
        status_icon.set_pixel_size(16);
        let usage_label = gtk::Label::builder()
            .label("Context unavailable")
            .visible(false)
            .tooltip_text("Token usage is not available yet")
            .build();
        let usage_icon = gtk::Image::from_icon_name("utilities-system-monitor-symbolic");
        usage_icon.set_pixel_size(16);
        usage_icon.set_tooltip_text(Some("Context usage is not available yet"));
        let usage_progress = gtk::ProgressBar::builder()
            .fraction(0.0)
            .show_text(true)
            .text("—")
            .width_request(72)
            .valign(gtk::Align::Center)
            .tooltip_text("Token usage is not available yet")
            .build();
        let thread_menu_button = thread_command_menu(callbacks.clone());
        thread_menu_button.set_sensitive(false);
        let tools_menu_button = codex_tools_menu(callbacks.clone());
        tools_menu_button.set_sensitive(false);

        let (primary_selectors, secondary_selectors, selectors) = selector_controls();
        let primary_toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(4)
            .margin_bottom(2)
            .margin_start(8)
            .margin_end(8)
            .build();
        primary_toolbar.append(&status_icon);
        primary_toolbar.append(&status_label);
        primary_toolbar.append(&usage_icon);
        primary_toolbar.append(&usage_progress);
        primary_toolbar.append(&primary_selectors);
        let toolbar_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toolbar_spacer.set_hexpand(true);
        primary_toolbar.append(&toolbar_spacer);
        primary_toolbar.append(&tools_menu_button);
        primary_toolbar.append(&thread_menu_button);
        let secondary_toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(2)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        secondary_toolbar.append(&secondary_selectors);

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        header.append(&primary_toolbar);
        header.append(&secondary_toolbar);

        let plan_progress_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .visible(false)
            .build();
        let collaboration_progress_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .visible(false)
            .build();
        let progress_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        progress_content.add_css_class("card");
        progress_content.append(&plan_progress_box);
        progress_content.append(&collaboration_progress_box);
        let progress_clamp = adw::Clamp::builder()
            .maximum_size(960)
            .tightening_threshold(720)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .child(&progress_content)
            .build();
        let progress_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .child(&progress_clamp)
            .build();

        let composer = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .left_margin(10)
            .right_margin(10)
            .top_margin(10)
            .bottom_margin(10)
            .hexpand(true)
            .vexpand(true)
            .build();
        let composer_placeholder = gtk::Label::builder()
            .label("Message Codex…")
            .css_classes(["dim-label"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .margin_start(14)
            .can_target(false)
            .build();
        let composer_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&composer)
            .build();
        let composer_overlay = gtk::Overlay::new();
        composer_overlay.set_child(Some(&composer_scroller));
        composer_overlay.add_overlay(&composer_placeholder);
        let composer_frame = gtk::Frame::builder()
            .hexpand(true)
            .child(&composer_overlay)
            .build();

        let queued_submissions_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .show_separators(true)
            .build();
        queued_submissions_list.add_css_class("boxed-list");
        let queued_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(136)
            .child(&queued_submissions_list)
            .build();
        let queue_heading = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        let queue_icon = gtk::Image::from_icon_name("view-list-symbolic");
        queue_icon.set_pixel_size(14);
        queue_heading.append(&queue_icon);
        queue_heading.append(
            &gtk::Label::builder()
                .label("Queued follow-ups")
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .build(),
        );
        let queued_submissions_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .visible(false)
            .build();
        queued_submissions_box.append(&queue_heading);
        queued_submissions_box.append(&queued_scroller);

        let attachment_flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(4)
            .row_spacing(4)
            .max_children_per_line(16)
            .visible(false)
            .build();

        let context_button = add_context_menu(callbacks.clone());
        let send_button = gtk::Button::builder()
            .label("Send")
            .tooltip_text("Send (Enter)")
            .sensitive(false)
            .build();
        send_button.add_css_class("suggested-action");
        let steer_button = gtk::Button::builder()
            .icon_name("mail-send-symbolic")
            .tooltip_text("Add instructions to the active turn (Enter)")
            .visible(false)
            .build();
        steer_button.update_property(&[gtk::accessible::Property::Label("Steer active turn")]);
        steer_button.add_css_class("flat");
        let interrupt_button = gtk::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Interrupt the active turn")
            .visible(false)
            .build();
        interrupt_button
            .update_property(&[gtk::accessible::Property::Label("Interrupt active turn")]);
        interrupt_button.add_css_class("destructive-action");

        let input_actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        input_actions.append(&context_button);
        let turn_actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        turn_actions.append(&interrupt_button);
        turn_actions.append(&steer_button);
        turn_actions.append(&send_button);
        let action_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        action_row.append(&input_actions);
        let action_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        action_spacer.set_hexpand(true);
        action_row.append(&action_spacer);
        action_row.append(&turn_actions);

        let composer_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        composer_content.append(&queued_submissions_box);
        composer_content.append(&attachment_flow);
        composer_content.append(&composer_frame);
        composer_content.append(&action_row);
        let composer_clamp = adw::Clamp::builder()
            .maximum_size(960)
            .tightening_threshold(720)
            .hexpand(true)
            .child(&composer_content)
            .build();

        let transcript_area = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        transcript_area.append(&progress_revealer);
        transcript_area.append(&transcript_panel);
        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .wide_handle(false)
            .resize_start_child(true)
            .resize_end_child(false)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .start_child(&transcript_area)
            .end_child(&composer_clamp)
            .hexpand(true)
            .vexpand(true)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.add_css_class("codex-native-chat");
        apply_chat_font_size(config::load().font_sizes.agent);
        root.append(&header);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&split);

        let state = Rc::new(CodexChatViewState {
            model,
            timeline_indices: RefCell::new(HashMap::new()),
            pending_indices: RefCell::new(HashMap::new()),
            queued_timeline_updates: RefCell::new(Vec::new()),
            timeline_flush_scheduled: Cell::new(false),
            transcript_stack,
            transcript_scroller,
            status_icon,
            status_label,
            usage_label,
            usage_icon,
            usage_progress,
            older_turns_row,
            older_turns_button,
            older_turns_spinner,
            older_turns_available: Cell::new(false),
            header_action_widgets: vec![tools_menu_button.upcast(), thread_menu_button.upcast()],
            selectors,
            progress_revealer,
            plan_progress_box,
            collaboration_progress_box,
            composer,
            composer_placeholder,
            queued_submissions_box,
            queued_submissions_list,
            attachment_flow,
            attachments: RefCell::new(Vec::new()),
            send_button,
            steer_button,
            interrupt_button,
            callbacks,
            connected: Cell::new(false),
            turn_active: Cell::new(false),
            composer_allowed: Cell::new(true),
        });

        connect_selector_controls(&state);
        connect_composer(&state, &composer_frame);
        connect_font_size_shortcuts(&root);

        Self { root, state }
    }

    pub fn connect_action<F>(&self, callback: F)
    where
        F: Fn(CodexChatAction) + 'static,
    {
        self.state.callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub fn set_connection_status(&self, status: ChatConnectionStatus) {
        self.state.status_label.remove_css_class("error");
        let show_status = !matches!(&status, ChatConnectionStatus::Ready);
        let (label, icon_name, connected) = match status {
            ChatConnectionStatus::Disconnected => (
                "Disconnected".to_string(),
                "network-offline-symbolic",
                false,
            ),
            ChatConnectionStatus::Connecting => (
                "Connecting to Codex App Server…".to_string(),
                "network-transmit-receive-symbolic",
                false,
            ),
            ChatConnectionStatus::Initializing => (
                "Initializing Codex App Server…".to_string(),
                "content-loading-symbolic",
                false,
            ),
            ChatConnectionStatus::Ready => {
                (String::new(), "network-transmit-receive-symbolic", true)
            }
            ChatConnectionStatus::Failed(message) => {
                self.state.status_label.add_css_class("error");
                (
                    format!("Connection failed: {message}"),
                    "dialog-error-symbolic",
                    false,
                )
            }
        };
        self.state.status_icon.set_icon_name(Some(icon_name));
        self.state.status_icon.set_visible(show_status);
        self.state.status_label.set_text(&label);
        self.state.status_label.set_tooltip_text(Some(&label));
        self.state.status_label.set_visible(show_status);
        self.state.connected.set(connected);
        for widget in &self.state.header_action_widgets {
            widget.set_sensitive(connected);
        }
        update_action_sensitivity(&self.state);
    }

    pub fn set_turn_active(&self, active: bool) {
        self.state.turn_active.set(active);
        update_action_sensitivity(&self.state);
    }

    pub fn set_composer_enabled(&self, enabled: bool) {
        self.state.composer_allowed.set(enabled);
        self.state.composer.set_editable(enabled);
        update_action_sensitivity(&self.state);
    }

    pub fn set_composer_text(&self, text: &str) {
        self.state.composer.buffer().set_text(text);
    }

    pub fn focus_composer(&self) {
        self.state.composer.grab_focus();
    }

    pub fn set_queued_submissions(&self, submissions: &[QueuedSubmission]) {
        while let Some(child) = self.state.queued_submissions_list.first_child() {
            self.state.queued_submissions_list.remove(&child);
        }
        self.state
            .queued_submissions_box
            .set_visible(!submissions.is_empty());
        let last_index = submissions.len().saturating_sub(1);
        for (index, submission) in submissions.iter().enumerate() {
            let row_content = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .margin_top(3)
                .margin_bottom(3)
                .margin_start(6)
                .margin_end(4)
                .build();
            let icon = gtk::Image::from_icon_name("mail-send-symbolic");
            icon.set_pixel_size(12);
            let preview = gtk::Label::builder()
                .label(if submission.preview.trim().is_empty() {
                    "Queued follow-up"
                } else {
                    submission.preview.trim()
                })
                .tooltip_text(&submission.preview)
                .css_classes(["caption"])
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(52)
                .build();
            row_content.append(&icon);
            row_content.append(&preview);
            for (icon_name, tooltip, action, visible) in [
                (
                    "document-edit-symbolic",
                    "Edit queued follow-up",
                    CodexChatAction::EditQueued(submission.id.clone()),
                    true,
                ),
                (
                    "go-up-symbolic",
                    "Move queued follow-up up",
                    CodexChatAction::MoveQueued {
                        id: submission.id.clone(),
                        direction: QueueDirection::Up,
                    },
                    index > 0,
                ),
                (
                    "go-down-symbolic",
                    "Move queued follow-up down",
                    CodexChatAction::MoveQueued {
                        id: submission.id.clone(),
                        direction: QueueDirection::Down,
                    },
                    index < last_index,
                ),
                (
                    "user-trash-symbolic",
                    "Remove queued follow-up",
                    CodexChatAction::RemoveQueued(submission.id.clone()),
                    true,
                ),
            ] {
                let button = gtk::Button::builder()
                    .icon_name(icon_name)
                    .tooltip_text(tooltip)
                    .visible(visible)
                    .valign(gtk::Align::Center)
                    .build();
                button.add_css_class("flat");
                button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
                button.connect_clicked({
                    let callbacks = self.state.callbacks.clone();
                    move |_| emit_action(&callbacks, action.clone())
                });
                row_content.append(&button);
            }
            let row = gtk::ListBoxRow::builder()
                .activatable(false)
                .selectable(false)
                .child(&row_content)
                .build();
            self.state.queued_submissions_list.append(&row);
        }
    }

    pub fn restore_submission_for_editing(&self, submission: ComposerSubmission) {
        self.state.composer.buffer().set_text(&submission.text);
        self.state.attachments.replace(submission.attachments);
        sync_attachment_chips(&self.state);
        self.state.composer.grab_focus();
    }

    pub fn add_attachment(&self, attachment: ComposerAttachment) {
        let mut attachments = self.state.attachments.borrow_mut();
        if let Some(existing) = attachments
            .iter_mut()
            .find(|existing| existing.id == attachment.id)
        {
            *existing = attachment;
        } else {
            attachments.push(attachment);
        }
        drop(attachments);
        sync_attachment_chips(&self.state);
    }

    pub fn upsert_timeline_item(&self, item: TimelineItem) {
        self.state
            .queued_timeline_updates
            .borrow_mut()
            .retain(|queued| queued.id != item.id);
        let follow = adjustment_is_near_bottom(&self.state.transcript_scroller);
        let existing = self.state.timeline_indices.borrow().get(&item.id).copied();
        let object = glib::BoxedAnyObject::new(TranscriptEntry::Timeline(item.clone()));
        if let Some(position) = existing {
            self.state.model.splice(position, 1, &[object]);
        } else {
            let position = self.state.model.n_items();
            self.state.model.append(&object);
            self.state
                .timeline_indices
                .borrow_mut()
                .insert(item.id, position);
        }
        self.state
            .transcript_stack
            .set_visible_child_name("transcript");
        if follow || existing.is_none() {
            scroll_transcript_to_end(&self.state.transcript_scroller);
        }
    }

    pub fn queue_timeline_item(&self, item: TimelineItem) {
        let mut queued = self.state.queued_timeline_updates.borrow_mut();
        if let Some(existing) = queued.iter_mut().find(|queued| queued.id == item.id) {
            *existing = item;
        } else {
            queued.push(item);
        }
        drop(queued);
        if self.state.timeline_flush_scheduled.replace(true) {
            return;
        }
        let view = self.clone();
        glib::idle_add_local_once(move || {
            view.state.timeline_flush_scheduled.set(false);
            let updates = view.state.queued_timeline_updates.take();
            for item in updates {
                view.upsert_timeline_item(item);
            }
        });
    }

    pub fn upsert_pending_request(&self, request: PendingRequest) {
        let follow = adjustment_is_near_bottom(&self.state.transcript_scroller);
        let existing = self
            .state
            .pending_indices
            .borrow()
            .get(&request.request_id)
            .copied();
        let object = glib::BoxedAnyObject::new(TranscriptEntry::Pending(request.clone()));
        if let Some(position) = existing {
            self.state.model.splice(position, 1, &[object]);
        } else {
            let position = self.state.model.n_items();
            self.state.model.append(&object);
            self.state
                .pending_indices
                .borrow_mut()
                .insert(request.request_id, position);
        }
        self.state
            .transcript_stack
            .set_visible_child_name("transcript");
        if follow || existing.is_none() {
            scroll_transcript_to_end(&self.state.transcript_scroller);
        }
    }

    pub fn resolve_pending_request(&self, request_id: &str) {
        let Some(position) = self.state.pending_indices.borrow().get(request_id).copied() else {
            return;
        };
        self.state.model.remove(position);
        rebuild_entry_indices(&self.state);
        sync_transcript_stack(&self.state);
    }

    pub fn clear_timeline(&self) {
        self.state.model.remove_all();
        self.state.timeline_indices.borrow_mut().clear();
        self.state.pending_indices.borrow_mut().clear();
        self.state.queued_timeline_updates.borrow_mut().clear();
        self.state.transcript_stack.set_visible_child_name("empty");
    }

    pub fn prepend_timeline_items(&self, items: &[TimelineItem]) {
        let known = self.state.timeline_indices.borrow();
        let objects = items
            .iter()
            .filter(|item| !known.contains_key(&item.id))
            .cloned()
            .map(|item| glib::BoxedAnyObject::new(TranscriptEntry::Timeline(item)))
            .collect::<Vec<_>>();
        drop(known);
        if objects.is_empty() {
            return;
        }
        let adjustment = self.state.transcript_scroller.vadjustment();
        let previous_upper = adjustment.upper();
        let previous_value = adjustment.value();
        self.state.model.splice(0, 0, &objects);
        rebuild_entry_indices(&self.state);
        self.state
            .transcript_stack
            .set_visible_child_name("transcript");
        glib::idle_add_local_once(move || {
            let added_height = (adjustment.upper() - previous_upper).max(0.0);
            let maximum = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment
                .set_value((previous_value + added_height).clamp(adjustment.lower(), maximum));
        });
    }

    pub fn set_older_turns_available(&self, available: bool) {
        self.state.older_turns_available.set(available);
        if !self.state.older_turns_spinner.is_visible() {
            self.state.older_turns_row.set_visible(available);
        }
    }

    pub fn set_older_turns_loading(&self, loading: bool) {
        self.state.older_turns_spinner.set_visible(loading);
        self.state
            .older_turns_row
            .set_visible(loading || self.state.older_turns_available.get());
        self.state.older_turns_button.set_sensitive(!loading);
        self.state.older_turns_button.set_label(if loading {
            "Loading older messages…"
        } else {
            "Load older messages"
        });
    }

    pub fn set_usage(&self, usage: Option<TokenUsage>) {
        let Some(usage) = usage else {
            self.state.usage_label.set_text("Context unavailable");
            self.state
                .usage_label
                .set_tooltip_text(Some("Token usage is not available yet"));
            self.state
                .usage_icon
                .set_tooltip_text(Some("Context usage is not available yet"));
            self.state.usage_progress.set_fraction(0.0);
            self.state.usage_progress.set_text(Some("—"));
            self.state
                .usage_progress
                .set_tooltip_text(Some("Token usage is not available yet"));
            return;
        };

        let context = usage.context_limit.filter(|limit| *limit > 0);
        const BASELINE_TOKENS: u64 = 12_000;
        let remaining = context.map(|limit| {
            let effective_window = limit.saturating_sub(BASELINE_TOKENS);
            let used = usage.last_total_tokens.saturating_sub(BASELINE_TOKENS);
            let remaining = effective_window.saturating_sub(used);
            let fraction = if effective_window == 0 {
                0.0
            } else {
                (remaining as f64 / effective_window as f64).clamp(0.0, 1.0)
            };
            (remaining, effective_window, fraction)
        });
        let summary = remaining.map_or_else(
            || "Context remaining: unknown".to_owned(),
            |(_, _, fraction)| format!("{:.0}% context remaining", fraction * 100.0),
        );
        let remaining_detail = remaining.map_or_else(
            || "Context remaining: unknown".to_owned(),
            |(remaining, effective_window, fraction)| {
                format!(
                    "Context remaining: {remaining} / {effective_window} tokens ({:.0}%)",
                    fraction * 100.0
                )
            },
        );
        let detail = format!(
            "{remaining_detail}\nActive context: {}{}\n\nCumulative usage\nInput: {}\nCache write input: {}\nCached input: {}\nOutput: {}\nReasoning output: {}\nTotal: {}",
            usage.last_total_tokens,
            context.map_or_else(String::new, |limit| format!(" / {limit}")),
            usage.input_tokens,
            usage.cache_write_input_tokens,
            usage.cached_input_tokens,
            usage.output_tokens,
            usage.reasoning_output_tokens,
            usage.total_tokens,
        );
        let fraction = remaining.map_or(0.0, |(_, _, fraction)| fraction);
        self.state.usage_label.set_text(&summary);
        self.state.usage_label.set_tooltip_text(Some(&detail));
        self.state.usage_icon.set_tooltip_text(Some(&detail));
        self.state.usage_progress.set_fraction(fraction);
        self.state
            .usage_progress
            .set_text(Some(&remaining.map_or_else(
                || "—".to_owned(),
                |(_, _, fraction)| format!("{:.0}%", fraction * 100.0),
            )));
        self.state.usage_progress.set_tooltip_text(Some(&detail));
    }

    pub fn set_plan_progress(&self, progress: Option<PlanProgress>) {
        clear_box(&self.state.plan_progress_box);
        let Some(progress) = progress else {
            self.state.plan_progress_box.set_visible(false);
            sync_progress_visibility(&self.state);
            return;
        };
        self.state.plan_progress_box.append(
            &gtk::Label::builder()
                .label(progress.title.as_deref().unwrap_or("Plan"))
                .css_classes(["heading"])
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
        if let Some(summary) = progress
            .summary
            .as_deref()
            .filter(|summary| !summary.is_empty())
        {
            self.state.plan_progress_box.append(
                &gtk::Label::builder()
                    .label(summary)
                    .css_classes(["dim-label"])
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .build(),
            );
        }
        let completed = progress
            .steps
            .iter()
            .filter(|step| step.status == PlanStepStatus::Completed)
            .count();
        let step_count = progress.steps.len();
        let progress_text = if step_count == 0 {
            "No plan steps".to_string()
        } else {
            format!("{completed} of {step_count} complete")
        };
        let progress_bar = gtk::ProgressBar::builder()
            .fraction(if step_count == 0 {
                0.0
            } else {
                completed as f64 / step_count as f64
            })
            .show_text(true)
            .text(&progress_text)
            .build();
        self.state.plan_progress_box.append(&progress_bar);
        for step in progress.steps {
            self.state.plan_progress_box.append(&progress_row(
                &step.label,
                step.detail.as_deref(),
                match step.status {
                    PlanStepStatus::Pending => ("radio-symbolic", None),
                    PlanStepStatus::InProgress => ("media-playback-start-symbolic", None),
                    PlanStepStatus::Completed => ("emblem-ok-symbolic", None),
                    PlanStepStatus::Failed => ("dialog-error-symbolic", Some("error")),
                },
            ));
        }
        self.state.plan_progress_box.set_visible(true);
        sync_progress_visibility(&self.state);
    }

    pub fn set_collaboration_progress(&self, progress: Option<CollaborationProgress>) {
        clear_box(&self.state.collaboration_progress_box);
        let Some(progress) = progress else {
            self.state.collaboration_progress_box.set_visible(false);
            sync_progress_visibility(&self.state);
            return;
        };
        self.state.collaboration_progress_box.append(
            &gtk::Label::builder()
                .label(progress.title.as_deref().unwrap_or("Collaboration"))
                .css_classes(["heading"])
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
        for participant in progress.participants {
            self.state.collaboration_progress_box.append(&progress_row(
                &participant.label,
                participant.detail.as_deref(),
                match participant.status {
                    CollaborationParticipantStatus::Pending => ("radio-symbolic", None),
                    CollaborationParticipantStatus::Working => {
                        ("media-playback-start-symbolic", None)
                    }
                    CollaborationParticipantStatus::Completed => ("emblem-ok-symbolic", None),
                    CollaborationParticipantStatus::Failed => {
                        ("dialog-error-symbolic", Some("error"))
                    }
                },
            ));
        }
        self.state.collaboration_progress_box.set_visible(true);
        sync_progress_visibility(&self.state);
    }

    pub fn set_selector_options(
        &self,
        selector: ChatSelector,
        options: &[SelectorOption],
        selected_id: Option<&str>,
    ) {
        let Some(control) = self.state.selectors.get(&selector) else {
            return;
        };
        control.updating.set(true);
        control
            .ids
            .replace(options.iter().map(|option| option.id.clone()).collect());
        let labels = options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>();
        let model = gtk::StringList::new(&labels);
        control.dropdown.set_model(Some(&model));
        let selected = selected_id
            .and_then(|selected_id| options.iter().position(|option| option.id == selected_id))
            .map_or(gtk::INVALID_LIST_POSITION, |position| position as u32);
        control.dropdown.set_selected(selected);
        control.dropdown.set_sensitive(!options.is_empty());
        control.updating.set(false);
    }
}

// Thread and context popovers live in `menus.rs`.
fn transcript_factory(callbacks: Rc<RefCell<Vec<ActionCallback>>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(entry) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            item.set_child(None::<&gtk::Widget>);
            return;
        };
        let entry = entry.borrow::<TranscriptEntry>().clone();
        let row = match entry {
            TranscriptEntry::Timeline(entry) => timeline_row(&entry),
            TranscriptEntry::Pending(request) => pending_request_row(&request, &callbacks),
        };
        item.set_child(Some(&row));
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(None::<&gtk::Widget>);
        }
    });
    factory
}

// Transcript and pending-request widgets live in `transcript.rs`.
fn selector_controls() -> (gtk::Box, gtk::Box, HashMap<ChatSelector, SelectorControl>) {
    let primary = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    let secondary = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    let mut controls = HashMap::new();
    for selector in CHAT_SELECTORS {
        let (label, icon_name) = match selector {
            ChatSelector::Model => ("Model", "computer-symbolic"),
            ChatSelector::Reasoning => ("Reasoning effort", "applications-science-symbolic"),
            ChatSelector::ReasoningSummary => ("Reasoning summary", "view-list-symbolic"),
            ChatSelector::Personality => ("Personality", "avatar-default-symbolic"),
            ChatSelector::Permissions => ("Permissions", "security-high-symbolic"),
            ChatSelector::Collaboration => ("Collaboration", "system-users-symbolic"),
            ChatSelector::ServiceTier => ("Response speed", "media-seek-forward-symbolic"),
            ChatSelector::ApprovalReviewer => ("Approval reviewer", "emblem-ok-symbolic"),
        };
        let dropdown = gtk::DropDown::builder()
            .enable_search(true)
            .sensitive(false)
            .tooltip_text(label)
            .build();
        dropdown.set_size_request(
            if selector == ChatSelector::ServiceTier {
                104
            } else {
                116
            },
            -1,
        );
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        icon.set_tooltip_text(Some(label));
        let control_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .tooltip_text(label)
            .build();
        control_box.append(&icon);
        control_box.append(&dropdown);
        if matches!(
            selector,
            ChatSelector::Model
                | ChatSelector::Reasoning
                | ChatSelector::Personality
                | ChatSelector::Permissions
        ) {
            primary.append(&control_box);
        } else {
            secondary.append(&control_box);
        }
        controls.insert(
            selector,
            SelectorControl {
                dropdown,
                ids: RefCell::new(Vec::new()),
                updating: Cell::new(false),
            },
        );
    }
    (primary, secondary, controls)
}

fn connect_selector_controls(state: &Rc<CodexChatViewState>) {
    for selector in CHAT_SELECTORS {
        let Some(control) = state.selectors.get(&selector) else {
            continue;
        };
        control.dropdown.connect_selected_notify({
            let state = Rc::downgrade(state);
            move |dropdown| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let Some(control) = state.selectors.get(&selector) else {
                    return;
                };
                if control.updating.get() {
                    return;
                }
                let position = dropdown.selected();
                let value = if position == gtk::INVALID_LIST_POSITION {
                    None
                } else {
                    control.ids.borrow().get(position as usize).cloned()
                };
                emit_action(
                    &state.callbacks,
                    CodexChatAction::SelectorChanged { selector, value },
                );
            }
        });
    }
}

fn apply_chat_font_size(font_size: f64) {
    CHAT_FONT_PROVIDER.with(|provider| {
        provider.load_from_data(&format!(
            ".codex-native-chat {{ font-size: {font_size:.1}pt; }}"
        ));
    });
}

fn connect_font_size_shortcuts(root: &gtk::Box) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            || modifiers.contains(gdk::ModifierType::ALT_MASK)
        {
            return glib::Propagation::Proceed;
        }
        let current = config::load().font_sizes.agent;
        let requested = if matches!(key, gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add) {
            current + 1.0
        } else if matches!(
            key,
            gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract
        ) {
            current - 1.0
        } else if matches!(key, gdk::Key::_0 | gdk::Key::KP_0) {
            config::DEFAULT_AGENT_FONT_SIZE
        } else {
            return glib::Propagation::Proceed;
        };
        let next = config::normalize_font_size(requested, config::DEFAULT_AGENT_FONT_SIZE);
        if (next - current).abs() > f64::EPSILON {
            apply_chat_font_size(next);
            config::save_agent_font_size(next);
        }
        glib::Propagation::Stop
    });
    root.add_controller(keys);
}

fn connect_composer(state: &Rc<CodexChatViewState>, drop_widget: &gtk::Frame) {
    state.composer.buffer().connect_changed({
        let state = Rc::downgrade(state);
        move |_| {
            let Some(state) = state.upgrade() else {
                return;
            };
            update_action_sensitivity(&state);
        }
    });
    state.send_button.connect_clicked({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                submit_composer(&state, false);
            }
        }
    });
    state.steer_button.connect_clicked({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                submit_composer(&state, true);
            }
        }
    });
    state.interrupt_button.connect_clicked({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                emit_action(&state.callbacks, CodexChatAction::Interrupt);
            }
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let state = Rc::downgrade(state);
        move |_, key, _, modifiers| {
            if matches!(key, gdk::Key::v | gdk::Key::V)
                && modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            {
                let Some(state) = state.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !state.connected.get() || !state.composer_allowed.get() {
                    return glib::Propagation::Proceed;
                }
                let clipboard = state.composer.clipboard();
                let formats = clipboard.formats();
                let has_image = formats.contains_type(gdk::Texture::static_type())
                    || formats
                        .mime_types()
                        .iter()
                        .any(|mime_type| mime_type.starts_with("image/"));
                if has_image {
                    let callbacks = state.callbacks.clone();
                    clipboard.read_texture_async(None::<&gio::Cancellable>, move |result| {
                        match result {
                            Ok(Some(texture)) => emit_action(
                                &callbacks,
                                CodexChatAction::PastedClipboardImage {
                                    png_bytes: texture.save_to_png_bytes().as_ref().to_vec(),
                                },
                            ),
                            Ok(None) => {}
                            Err(error) => {
                                log::warn!("failed reading pasted clipboard image: {error}")
                            }
                        }
                    });
                    return glib::Propagation::Stop;
                }
            }
            if !matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                || modifiers.contains(gdk::ModifierType::SHIFT_MASK)
            {
                return glib::Propagation::Proceed;
            }
            let Some(state) = state.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !state.connected.get() || !state.composer_allowed.get() {
                return glib::Propagation::Proceed;
            }
            submit_composer(&state, state.turn_active.get());
            glib::Propagation::Stop
        }
    });
    state.composer.add_controller(keys);

    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    drop.connect_drop({
        let state = Rc::downgrade(state);
        move |_, value, _, _| {
            let Some(state) = state.upgrade() else {
                return false;
            };
            let Ok(files) = value.get::<gdk::FileList>() else {
                return false;
            };
            let references = files
                .files()
                .into_iter()
                .map(|file| {
                    file.path()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| file.uri().to_string())
                })
                .collect::<Vec<_>>();
            if references.is_empty() {
                return false;
            }
            emit_action(&state.callbacks, CodexChatAction::FilesDropped(references));
            true
        }
    });
    drop_widget.add_controller(drop);
}

fn submit_composer(state: &Rc<CodexChatViewState>, steer: bool) {
    if !state.connected.get() || !state.composer_allowed.get() {
        return;
    }
    if steer && !state.turn_active.get() {
        return;
    }
    let buffer = state.composer.buffer();
    let text = buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .trim()
        .to_string();
    let attachments = state.attachments.borrow().clone();
    if text.is_empty() && attachments.is_empty() {
        return;
    }
    let submission = ComposerSubmission { text, attachments };
    emit_action(
        &state.callbacks,
        if steer {
            CodexChatAction::Steer(submission)
        } else if state.turn_active.get() {
            CodexChatAction::Queue(submission)
        } else {
            CodexChatAction::Submit(submission)
        },
    );
    buffer.set_text("");
    state.attachments.borrow_mut().clear();
    sync_attachment_chips(state);
}

fn update_action_sensitivity(state: &CodexChatViewState) {
    let buffer = state.composer.buffer();
    let has_text = !buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .trim()
        .is_empty();
    let has_input = has_text || !state.attachments.borrow().is_empty();
    let can_submit = state.connected.get() && state.composer_allowed.get() && has_input;
    state.send_button.set_sensitive(can_submit);
    state
        .send_button
        .set_tooltip_text(Some(if state.turn_active.get() {
            "Queue follow-up"
        } else {
            "Send (Ctrl+Enter)"
        }));
    let turn_controls_visible = state.connected.get() && state.turn_active.get();
    let can_steer = can_submit && state.turn_active.get();
    state.steer_button.set_visible(can_steer);
    state.steer_button.set_sensitive(can_steer);
    state.interrupt_button.set_visible(turn_controls_visible);
    state
        .interrupt_button
        .set_sensitive(state.connected.get() && state.turn_active.get());
    state.composer_placeholder.set_visible(!has_text);
}

fn sync_attachment_chips(state: &Rc<CodexChatViewState>) {
    while let Some(child) = state.attachment_flow.first_child() {
        state.attachment_flow.remove(&child);
    }
    let attachments = state.attachments.borrow().clone();
    state.attachment_flow.set_visible(!attachments.is_empty());
    for attachment in attachments {
        let reference_path = std::path::Path::new(&attachment.reference);
        let workspace_folder =
            matches!(&attachment.kind, ComposerAttachmentKind::Mention) && reference_path.is_dir();
        let icon_name = match &attachment.kind {
            ComposerAttachmentKind::File => "text-x-generic-symbolic",
            ComposerAttachmentKind::Image => "image-x-generic-symbolic",
            ComposerAttachmentKind::Audio => "audio-x-generic-symbolic",
            ComposerAttachmentKind::Mention if workspace_folder => "folder-open-symbolic",
            ComposerAttachmentKind::Mention => "document-open-symbolic",
            ComposerAttachmentKind::Skill => "applications-education-symbolic",
        };
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(12);
        let basename = reference_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&attachment.label);
        let display_label = if matches!(&attachment.kind, ComposerAttachmentKind::Skill) {
            attachment.label.as_str()
        } else {
            basename
        };
        let label = gtk::Label::builder()
            .label(display_label)
            .css_classes(["caption"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(16)
            .build();
        let remove_icon = gtk::Image::from_icon_name("window-close-symbolic");
        remove_icon.set_pixel_size(12);
        let remove = gtk::Button::builder()
            .child(&remove_icon)
            .tooltip_text("Remove context")
            .valign(gtk::Align::Center)
            .build();
        remove.add_css_class("flat");
        remove.add_css_class("circular");
        remove.update_property(&[gtk::accessible::Property::Label("Remove context")]);
        remove.connect_clicked({
            let state = Rc::downgrade(state);
            let attachment_id = attachment.id.clone();
            move |_| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                state
                    .attachments
                    .borrow_mut()
                    .retain(|attachment| attachment.id != attachment_id);
                emit_action(
                    &state.callbacks,
                    CodexChatAction::AttachmentRemoved(attachment_id.clone()),
                );
                sync_attachment_chips(&state);
            }
        });
        let context_kind = match &attachment.kind {
            ComposerAttachmentKind::Image => "Image attachment",
            ComposerAttachmentKind::Audio => "Audio attachment",
            ComposerAttachmentKind::Mention if workspace_folder => "Workspace folder reference",
            ComposerAttachmentKind::Mention | ComposerAttachmentKind::File => {
                "Workspace file reference"
            }
            ComposerAttachmentKind::Skill => "Skill",
        };
        let tooltip = if matches!(&attachment.kind, ComposerAttachmentKind::Skill) {
            format!("Skill: {}", attachment.label)
        } else {
            format!("{context_kind}\n{}", attachment.reference)
        };
        let chip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .tooltip_text(&tooltip)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Start)
            .build();
        chip.add_css_class("pill");
        chip.append(&icon);
        chip.append(&label);
        chip.append(&remove);
        state.attachment_flow.insert(&chip, -1);
    }
    update_action_sensitivity(state);
}

fn rebuild_entry_indices(state: &CodexChatViewState) {
    let mut timeline = HashMap::new();
    let mut pending = HashMap::new();
    for position in 0..state.model.n_items() {
        let Some(entry) = state
            .model
            .item(position)
            .and_downcast::<glib::BoxedAnyObject>()
        else {
            continue;
        };
        match &*entry.borrow::<TranscriptEntry>() {
            TranscriptEntry::Timeline(item) => {
                timeline.insert(item.id.clone(), position);
            }
            TranscriptEntry::Pending(request) => {
                pending.insert(request.request_id.clone(), position);
            }
        }
    }
    state.timeline_indices.replace(timeline);
    state.pending_indices.replace(pending);
}

fn progress_row(
    label: &str,
    detail: Option<&str>,
    (icon_name, css_class): (&str, Option<&str>),
) -> gtk::Box {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    icon.set_valign(gtk::Align::Start);
    if let Some(css_class) = css_class {
        icon.add_css_class(css_class);
    }
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    labels.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build(),
    );
    if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
        labels.append(
            &gtk::Label::builder()
                .label(detail)
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build(),
        );
    }
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row.append(&icon);
    row.append(&labels);
    row
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn sync_progress_visibility(state: &CodexChatViewState) {
    state.progress_revealer.set_reveal_child(
        state.plan_progress_box.is_visible() || state.collaboration_progress_box.is_visible(),
    );
}

fn sync_transcript_stack(state: &CodexChatViewState) {
    state
        .transcript_stack
        .set_visible_child_name(if state.model.n_items() == 0 {
            "empty"
        } else {
            "transcript"
        });
}

fn adjustment_is_near_bottom(scroller: &gtk::ScrolledWindow) -> bool {
    let adjustment = scroller.vadjustment();
    adjustment.upper() - (adjustment.value() + adjustment.page_size()) <= TRANSCRIPT_FOLLOW_DISTANCE
}

fn scroll_transcript_to_end(scroller: &gtk::ScrolledWindow) {
    let scroller = scroller.clone();
    glib::idle_add_local_once(move || {
        let adjustment = scroller.vadjustment();
        adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(adjustment.lower()));
    });
}

fn emit_action(callbacks: &RefCell<Vec<ActionCallback>>, action: CodexChatAction) {
    for callback in callbacks.borrow().clone() {
        callback(action.clone());
    }
}
