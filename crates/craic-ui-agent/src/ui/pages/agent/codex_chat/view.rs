use super::{
    ChatConnectionStatus, ChatSelector, ChatTimelineEntry, CodexChatAction,
    CollaborationParticipantStatus, CollaborationProgress, ComposerAttachment,
    ComposerAttachmentKind, ComposerSubmission, DynamicToolOutputContent, DynamicToolRequest,
    McpElicitationResponseAction, McpFormField, McpFormFieldKind, McpFormRequest, McpUrlRequest,
    PendingRequest, PendingRequestKind, PendingRequestResponse, PlanProgress, PlanStepStatus,
    RequestOptionStyle, RequestSelectionMode, RequestUserInput, RequestUserInputAnswer,
    RequestUserInputQuestion, SelectorOption, StructuredRequestOption, StructuredRequestResponse,
    TimelineItem, TimelineItemKind, TimelineItemStatus, TokenUsage,
};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

const TRANSCRIPT_FOLLOW_DISTANCE: f64 = 72.0;

type ActionCallback = Rc<dyn Fn(CodexChatAction)>;

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
    transcript_stack: gtk::Stack,
    transcript_scroller: gtk::ScrolledWindow,
    status_icon: gtk::Image,
    status_label: gtk::Label,
    usage_label: gtk::Label,
    usage_progress: gtk::ProgressBar,
    header_action_widgets: Vec<gtk::Widget>,
    selectors: HashMap<ChatSelector, SelectorControl>,
    progress_revealer: gtk::Revealer,
    plan_progress_box: gtk::Box,
    collaboration_progress_box: gtk::Box,
    composer: gtk::TextView,
    composer_placeholder: gtk::Label,
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

        let status_label = gtk::Label::builder()
            .label("Disconnected")
            .css_classes(["dim-label"])
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let status_icon = gtk::Image::from_icon_name("network-offline-symbolic");
        status_icon.set_pixel_size(16);
        let usage_label = gtk::Label::builder()
            .label("Context: —")
            .css_classes(["caption", "dim-label"])
            .tooltip_text("Token usage is not available yet")
            .build();
        let usage_progress = gtk::ProgressBar::builder()
            .fraction(0.0)
            .width_request(96)
            .valign(gtk::Align::Center)
            .tooltip_text("Token usage is not available yet")
            .build();
        let new_thread_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New thread")
            .build();
        new_thread_button.add_css_class("flat");
        new_thread_button.set_sensitive(false);
        new_thread_button.connect_clicked({
            let callbacks = callbacks.clone();
            move |_| emit_action(&callbacks, CodexChatAction::NewThread)
        });
        let thread_menu_button = thread_command_menu(callbacks.clone());
        thread_menu_button.set_sensitive(false);
        let status_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(4)
            .margin_start(12)
            .margin_end(12)
            .build();
        status_row.append(&status_icon);
        status_row.append(&status_label);
        status_row.append(&usage_label);
        status_row.append(&usage_progress);
        status_row.append(&new_thread_button);
        status_row.append(&thread_menu_button);

        let (selector_row, selectors) = selector_controls();
        let selector_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .hexpand(true)
            .child(&selector_row)
            .build();

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        header.append(&status_row);
        header.append(&selector_scroller);

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
        composer.set_size_request(-1, 92);
        let composer_placeholder = gtk::Label::builder()
            .label("Message Codex…")
            .css_classes(["dim-label"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .margin_start(14)
            .can_target(false)
            .build();
        let composer_overlay = gtk::Overlay::new();
        composer_overlay.set_child(Some(&composer));
        composer_overlay.add_overlay(&composer_placeholder);
        let composer_frame = gtk::Frame::builder()
            .hexpand(true)
            .child(&composer_overlay)
            .build();

        let attachment_flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(12)
            .visible(false)
            .build();

        let attach_button = gtk::Button::builder()
            .icon_name("mail-attachment-symbolic")
            .tooltip_text("Attach a file")
            .build();
        attach_button.add_css_class("flat");
        let mention_button = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Mention a workspace file")
            .build();
        mention_button.add_css_class("flat");
        let send_button = gtk::Button::builder()
            .label("Send")
            .tooltip_text("Send (Ctrl+Enter)")
            .sensitive(false)
            .build();
        send_button.add_css_class("suggested-action");
        let steer_button = gtk::Button::builder()
            .label("Steer")
            .tooltip_text("Add instructions to the active turn (Ctrl+Enter)")
            .sensitive(false)
            .build();
        let interrupt_button = gtk::Button::builder()
            .label("Interrupt")
            .tooltip_text("Interrupt the active turn")
            .sensitive(false)
            .build();
        interrupt_button.add_css_class("destructive-action");

        let action_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        action_row.append(&attach_button);
        action_row.append(&mention_button);
        let action_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        action_spacer.set_hexpand(true);
        action_row.append(&action_spacer);
        action_row.append(&interrupt_button);
        action_row.append(&steer_button);
        action_row.append(&send_button);

        let composer_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        composer_content.append(&attachment_flow);
        composer_content.append(&composer_frame);
        composer_content.append(&action_row);
        let composer_clamp = adw::Clamp::builder()
            .maximum_size(960)
            .tightening_threshold(720)
            .hexpand(true)
            .child(&composer_content)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&progress_revealer);
        root.append(&transcript_stack);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&composer_clamp);

        let state = Rc::new(CodexChatViewState {
            model,
            timeline_indices: RefCell::new(HashMap::new()),
            pending_indices: RefCell::new(HashMap::new()),
            transcript_stack,
            transcript_scroller,
            status_icon,
            status_label,
            usage_label,
            usage_progress,
            header_action_widgets: vec![new_thread_button.upcast(), thread_menu_button.upcast()],
            selectors,
            progress_revealer,
            plan_progress_box,
            collaboration_progress_box,
            composer,
            composer_placeholder,
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
        connect_composer(&state, &attach_button, &mention_button, &composer_frame);

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
            ChatConnectionStatus::Ready => (
                "Connected".to_string(),
                "network-transmit-receive-symbolic",
                true,
            ),
            ChatConnectionStatus::Reconnecting => (
                "Reconnecting to Codex App Server…".to_string(),
                "view-refresh-symbolic",
                false,
            ),
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
        self.state.status_label.set_text(&label);
        self.state.status_label.set_tooltip_text(Some(&label));
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

    pub fn remove_attachment(&self, attachment_id: &str) {
        self.state
            .attachments
            .borrow_mut()
            .retain(|attachment| attachment.id != attachment_id);
        sync_attachment_chips(&self.state);
    }

    pub fn clear_attachments(&self) {
        self.state.attachments.borrow_mut().clear();
        sync_attachment_chips(&self.state);
    }

    pub fn upsert_timeline_item(&self, item: TimelineItem) {
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

    pub fn remove_timeline_item(&self, item_id: &str) {
        let Some(position) = self.state.timeline_indices.borrow().get(item_id).copied() else {
            return;
        };
        self.state.model.remove(position);
        rebuild_entry_indices(&self.state);
        sync_transcript_stack(&self.state);
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
        self.state.transcript_stack.set_visible_child_name("empty");
    }

    pub fn replace_timeline(&self, entries: &[ChatTimelineEntry]) {
        let follow = adjustment_is_near_bottom(&self.state.transcript_scroller);
        self.state.model.remove_all();
        self.state.timeline_indices.borrow_mut().clear();
        self.state.pending_indices.borrow_mut().clear();
        for entry in entries {
            let position = self.state.model.n_items();
            let entry = match entry {
                ChatTimelineEntry::Item(item) => {
                    self.state
                        .timeline_indices
                        .borrow_mut()
                        .insert(item.id.clone(), position);
                    TranscriptEntry::Timeline(item.clone())
                }
                ChatTimelineEntry::PendingRequest(request) => {
                    self.state
                        .pending_indices
                        .borrow_mut()
                        .insert(request.request_id.clone(), position);
                    TranscriptEntry::Pending(request.clone())
                }
            };
            self.state.model.append(&glib::BoxedAnyObject::new(entry));
        }
        sync_transcript_stack(&self.state);
        if follow && !entries.is_empty() {
            scroll_transcript_to_end(&self.state.transcript_scroller);
        }
    }

    pub fn set_usage(&self, usage: Option<TokenUsage>) {
        let Some(usage) = usage else {
            self.state.usage_label.set_text("Context: —");
            self.state
                .usage_label
                .set_tooltip_text(Some("Token usage is not available yet"));
            self.state.usage_progress.set_fraction(0.0);
            self.state
                .usage_progress
                .set_tooltip_text(Some("Token usage is not available yet"));
            return;
        };

        let context = usage.context_limit.filter(|limit| *limit > 0);
        let summary = context.map_or_else(
            || format!("Tokens: {}", usage.total_tokens),
            |limit| format!("Context: {} / {limit}", usage.total_tokens),
        );
        let detail = format!(
            "Input: {}\nCached input: {}\nOutput: {}\nReasoning output: {}\nTotal: {}{}",
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.output_tokens,
            usage.reasoning_output_tokens,
            usage.total_tokens,
            context.map_or_else(String::new, |limit| format!("\nContext limit: {limit}")),
        );
        let fraction = context.map_or(0.0, |limit| {
            (usage.total_tokens as f64 / limit as f64).clamp(0.0, 1.0)
        });
        self.state.usage_label.set_text(&summary);
        self.state.usage_label.set_tooltip_text(Some(&detail));
        self.state.usage_progress.set_fraction(fraction);
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

    pub fn selected_selector(&self, selector: ChatSelector) -> Option<String> {
        let control = self.state.selectors.get(&selector)?;
        let selected = control.dropdown.selected();
        if selected == gtk::INVALID_LIST_POSITION {
            return None;
        }
        control.ids.borrow().get(selected as usize).cloned()
    }
}

fn thread_command_menu(callbacks: Rc<RefCell<Vec<ActionCallback>>>) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    for (label, icon_name, action) in [
        (
            "Open thread…",
            "document-open-symbolic",
            CodexChatAction::OpenThread,
        ),
        (
            "Resume thread",
            "media-playback-start-symbolic",
            CodexChatAction::ResumeThread,
        ),
        (
            "Thread history",
            "document-open-recent-symbolic",
            CodexChatAction::ShowHistory,
        ),
        (
            "Fork thread",
            "vcs-branch-symbolic",
            CodexChatAction::ForkThread,
        ),
        (
            "Archive thread",
            "folder-symbolic",
            CodexChatAction::ArchiveThread,
        ),
        (
            "Compact context",
            "package-x-generic-symbolic",
            CodexChatAction::CompactThread,
        ),
        (
            "Start review",
            "emblem-ok-symbolic",
            CodexChatAction::StartReview,
        ),
        (
            "Roll back last turn",
            "edit-undo-symbolic",
            CodexChatAction::UndoLastTurn,
        ),
        (
            "Open changes / diff",
            "document-edit-symbolic",
            CodexChatAction::OpenChanges,
        ),
    ] {
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        row.append(&icon);
        row.append(&label);
        let command = gtk::Button::builder()
            .child(&row)
            .halign(gtk::Align::Fill)
            .build();
        command.add_css_class("flat");
        command.connect_clicked({
            let callbacks = callbacks.clone();
            let popover = popover.downgrade();
            move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                emit_action(&callbacks, action.clone());
            }
        });
        content.append(&command);
    }
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Thread actions")
        .popover(&popover)
        .build();
    button.add_css_class("flat");
    button
}

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

fn timeline_row(item: &TimelineItem) -> gtk::Widget {
    let (default_title, icon_name) = match &item.kind {
        TimelineItemKind::UserMessage => ("You", "avatar-default-symbolic"),
        TimelineItemKind::AssistantMessage => ("Codex", "system-run-symbolic"),
        TimelineItemKind::DeveloperMessage => ("Developer message", "dialog-information-symbolic"),
        TimelineItemKind::Reasoning => ("Reasoning", "brain-augemnted-symbolic"),
        TimelineItemKind::Plan => ("Plan", "view-list-symbolic"),
        TimelineItemKind::Command => ("Command", "utilities-terminal-symbolic"),
        TimelineItemKind::CommandOutput => ("Command output", "utilities-terminal-symbolic"),
        TimelineItemKind::FileChange => ("File change", "document-edit-symbolic"),
        TimelineItemKind::Tool => ("Tool", "applications-system-symbolic"),
        TimelineItemKind::McpTool => ("MCP tool", "network-server-symbolic"),
        TimelineItemKind::Web => ("Web", "web-browser-symbolic"),
        TimelineItemKind::Image => ("Image", "image-x-generic-symbolic"),
        TimelineItemKind::Audio => ("Audio", "audio-x-generic-symbolic"),
        TimelineItemKind::Collaboration => ("Collaboration", "system-users-symbolic"),
        TimelineItemKind::Review => ("Review", "emblem-ok-symbolic"),
        TimelineItemKind::Compaction => ("Context compacted", "package-x-generic-symbolic"),
        TimelineItemKind::Warning => ("Warning", "dialog-warning-symbolic"),
        TimelineItemKind::Error => ("Error", "dialog-error-symbolic"),
        TimelineItemKind::Unknown(kind) => (kind.as_str(), "dialog-question-symbolic"),
    };
    let title = item.title.as_deref().unwrap_or(default_title);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    icon.set_valign(gtk::Align::Center);
    let title_label = gtk::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let status_label = gtk::Label::builder()
        .label(match item.status {
            TimelineItemStatus::Pending => "Pending",
            TimelineItemStatus::Running => "Running",
            TimelineItemStatus::Completed => "Completed",
            TimelineItemStatus::Failed => "Failed",
            TimelineItemStatus::Interrupted => "Interrupted",
        })
        .css_classes(["caption", "dim-label"])
        .build();
    if matches!(item.status, TimelineItemStatus::Failed) {
        status_label.remove_css_class("dim-label");
        status_label.add_css_class("error");
    }
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.append(&icon);
    header.append(&title_label);
    header.append(&status_label);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&header);
    if !item.body.is_empty() {
        let body = gtk::Label::builder()
            .label(&item.body)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        if matches!(
            item.kind,
            TimelineItemKind::Command
                | TimelineItemKind::CommandOutput
                | TimelineItemKind::FileChange
        ) {
            body.add_css_class("monospace");
        }
        content.append(&body);
    }
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        let detail_label = gtk::Label::builder()
            .label(detail)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .css_classes(["monospace"])
            .build();
        let expander = gtk::Expander::builder()
            .label("Details")
            .child(&detail_label)
            .build();
        content.append(&expander);
    }

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    card.add_css_class("card");
    if matches!(item.kind, TimelineItemKind::Error) {
        card.add_css_class("error");
    }
    card.append(&content);
    let clamp = adw::Clamp::builder()
        .maximum_size(960)
        .tightening_threshold(720)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .child(&card)
        .build();
    clamp.upcast()
}

fn pending_request_row(
    request: &PendingRequest,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
) -> gtk::Widget {
    let icon_name = match &request.kind {
        PendingRequestKind::Approval => "dialog-question-symbolic",
        PendingRequestKind::UserInput | PendingRequestKind::StructuredUserInput(_) => {
            "input-keyboard-symbolic"
        }
        PendingRequestKind::McpElicitation
        | PendingRequestKind::McpForm(_)
        | PendingRequestKind::McpUrl(_) => "network-server-symbolic",
        PendingRequestKind::DynamicTool | PendingRequestKind::DynamicToolOutput(_) => {
            "applications-system-symbolic"
        }
        PendingRequestKind::TokenRefresh => "dialog-password-symbolic",
        PendingRequestKind::Unknown(_) => "dialog-question-symbolic",
    };
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    let title = gtk::Label::builder()
        .label(&request.title)
        .css_classes(["heading"])
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.append(&icon);
    header.append(&title);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&header);
    if !request.description.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(&request.description)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .build(),
        );
    }

    match &request.kind {
        PendingRequestKind::StructuredUserInput(input) => {
            append_user_input_request(&content, callbacks, &request.request_id, input)
        }
        PendingRequestKind::McpForm(form) => {
            append_mcp_form_request(&content, callbacks, &request.request_id, form)
        }
        PendingRequestKind::McpUrl(url) => {
            append_mcp_url_request(&content, callbacks, &request.request_id, url)
        }
        PendingRequestKind::DynamicToolOutput(tool) => {
            append_dynamic_tool_request(&content, callbacks, &request.request_id, tool)
        }
        _ => append_legacy_request_controls(&content, callbacks, request),
    }

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    card.add_css_class("card");
    card.append(&content);
    let clamp = adw::Clamp::builder()
        .maximum_size(960)
        .tightening_threshold(720)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .child(&card)
        .build();
    clamp.upcast()
}

fn append_legacy_request_controls(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request: &PendingRequest,
) {
    let choices = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(6)
        .build();
    for option in &request.options {
        let button = gtk::Button::with_label(&option.label);
        match option.style {
            RequestOptionStyle::Default => {}
            RequestOptionStyle::Suggested => button.add_css_class("suggested-action"),
            RequestOptionStyle::Destructive => button.add_css_class("destructive-action"),
        }
        button.connect_clicked({
            let callbacks = callbacks.clone();
            let content = content.downgrade();
            let request_id = request.request_id.clone();
            let option_id = option.id.clone();
            move |_| {
                let Some(content) = content.upgrade() else {
                    return;
                };
                content.set_sensitive(false);
                emit_action(
                    &callbacks,
                    CodexChatAction::ResolveRequest {
                        request_id: request_id.clone(),
                        response: PendingRequestResponse::Option(option_id.clone()),
                    },
                );
            }
        });
        choices.insert(&button, -1);
    }
    if !request.options.is_empty() {
        content.append(&choices);
    }

    if request.allows_text {
        let entry = gtk::Entry::builder()
            .placeholder_text(
                request
                    .text_placeholder
                    .as_deref()
                    .unwrap_or("Enter a response"),
            )
            .hexpand(true)
            .build();
        let submit = gtk::Button::with_label("Submit");
        submit.add_css_class("suggested-action");
        submit.set_sensitive(false);
        entry.connect_changed({
            let submit = submit.clone();
            move |entry| submit.set_sensitive(!entry.text().trim().is_empty())
        });
        submit.connect_clicked({
            let callbacks = callbacks.clone();
            let content = content.downgrade();
            let entry = entry.clone();
            let request_id = request.request_id.clone();
            move |_| {
                let Some(content) = content.upgrade() else {
                    return;
                };
                let response = entry.text().trim().to_string();
                if response.is_empty() {
                    return;
                }
                content.set_sensitive(false);
                emit_action(
                    &callbacks,
                    CodexChatAction::ResolveRequest {
                        request_id: request_id.clone(),
                        response: PendingRequestResponse::Text(response),
                    },
                );
            }
        });
        let response_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        response_row.append(&entry);
        response_row.append(&submit);
        content.append(&response_row);
    }
}

#[derive(Clone)]
enum QuestionResponseControl {
    Text {
        question_id: String,
        entry: gtk::Entry,
    },
    Choices {
        question_id: String,
        buttons: Vec<(gtk::CheckButton, String)>,
        other: Option<(gtk::CheckButton, gtk::Entry)>,
    },
}

impl QuestionResponseControl {
    fn answer(&self) -> Option<(String, RequestUserInputAnswer)> {
        match self {
            Self::Text { question_id, entry } => {
                let answer = entry.text().trim().to_owned();
                (!answer.is_empty()).then(|| {
                    (
                        question_id.clone(),
                        RequestUserInputAnswer {
                            answers: vec![answer],
                        },
                    )
                })
            }
            Self::Choices {
                question_id,
                buttons,
                other,
            } => {
                let mut answers = buttons
                    .iter()
                    .filter(|(button, _)| button.is_active())
                    .map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                if let Some((button, entry)) = other
                    && button.is_active()
                {
                    let answer = entry.text().trim();
                    if answer.is_empty() {
                        return None;
                    }
                    answers.push(answer.to_owned());
                }
                (!answers.is_empty()).then(|| {
                    (
                        question_id.clone(),
                        RequestUserInputAnswer { answers },
                    )
                })
            }
        }
    }

    fn connect_changed(&self, callback: Rc<dyn Fn()>) {
        match self {
            Self::Text { entry, .. } => entry.connect_changed(move |_| callback()).into(),
            Self::Choices { buttons, other, .. } => {
                for (button, _) in buttons {
                    button.connect_toggled({
                        let callback = callback.clone();
                        move |_| callback()
                    });
                }
                if let Some((button, entry)) = other {
                    button.connect_toggled({
                        let callback = callback.clone();
                        move |_| callback()
                    });
                    entry.connect_changed(move |_| callback());
                }
            }
        }
    }
}

fn append_user_input_request(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request_id: &str,
    input: &RequestUserInput,
) {
    let controls = input
        .questions
        .iter()
        .map(|question| append_user_input_question(content, question))
        .collect::<Rc<Vec<_>>>();
    let submit = gtk::Button::with_label("Submit answers");
    submit.add_css_class("suggested-action");
    let update_submit: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let submit = submit.clone();
        Rc::new(move || submit.set_sensitive(controls.iter().all(|control| control.answer().is_some())))
    };
    for control in controls.iter() {
        control.connect_changed(update_submit.clone());
    }
    update_submit();
    submit.connect_clicked({
        let callbacks = callbacks.clone();
        let content = content.downgrade();
        let controls = controls.clone();
        let request_id = request_id.to_owned();
        move |_| {
            let answers = controls
                .iter()
                .map(QuestionResponseControl::answer)
                .collect::<Option<BTreeMap<_, _>>>();
            let (Some(content), Some(answers)) = (content.upgrade(), answers) else {
                return;
            };
            resolve_structured_request(
                &content,
                &callbacks,
                &request_id,
                StructuredRequestResponse::UserInput { answers },
            );
        }
    });
    submit.set_halign(gtk::Align::End);
    content.append(&submit);
}

fn append_user_input_question(
    content: &gtk::Box,
    question: &RequestUserInputQuestion,
) -> QuestionResponseControl {
    let question_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    question_box.append(
        &gtk::Label::builder()
            .label(&question.header)
            .css_classes(["heading"])
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    question_box.append(
        &gtk::Label::builder()
            .label(&question.question)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build(),
    );
    let control = if question.options.is_empty() {
        let entry = request_text_entry(
            question.is_secret,
            Some(if question.is_secret {
                "Enter a private response"
            } else {
                "Enter a response"
            }),
        );
        question_box.append(&entry);
        QuestionResponseControl::Text {
            question_id: question.id.clone(),
            entry,
        }
    } else {
        let options_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let mut group = None::<gtk::CheckButton>;
        let mut buttons = Vec::new();
        for option in &question.options {
            let button = append_choice(&options_box, option);
            if question.selection_mode == RequestSelectionMode::Single {
                if let Some(group) = group.as_ref() {
                    button.set_group(Some(group));
                } else {
                    group = Some(button.clone());
                }
            }
            buttons.push((button, option.value.clone()));
        }
        let other = question.allows_other.then(|| {
            let button = gtk::CheckButton::with_label("Other");
            if question.selection_mode == RequestSelectionMode::Single {
                if let Some(group) = group.as_ref() {
                    button.set_group(Some(group));
                }
            }
            let entry = request_text_entry(question.is_secret, Some("Enter another response"));
            entry.set_sensitive(false);
            button.connect_toggled({
                let entry = entry.clone();
                move |button| entry.set_sensitive(button.is_active())
            });
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            row.append(&button);
            row.append(&entry);
            options_box.append(&row);
            (button, entry)
        });
        question_box.append(&options_box);
        QuestionResponseControl::Choices {
            question_id: question.id.clone(),
            buttons,
            other,
        }
    };
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.append(&question_box);
    content.append(&separator);
    control
}

fn append_choice(
    container: &gtk::Box,
    option: &StructuredRequestOption,
) -> gtk::CheckButton {
    let button = gtk::CheckButton::with_label(&option.label);
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    row.append(&button);
    if let Some(description) = option.description.as_deref().filter(|value| !value.is_empty()) {
        row.append(
            &gtk::Label::builder()
                .label(description)
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .margin_start(28)
                .build(),
        );
    }
    container.append(&row);
    button
}

fn request_text_entry(secret: bool, placeholder: Option<&str>) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder.unwrap_or("Enter a value"))
        .hexpand(true)
        .visibility(!secret)
        .input_purpose(if secret {
            gtk::InputPurpose::Password
        } else {
            gtk::InputPurpose::FreeForm
        })
        .build();
    if secret {
        entry.set_invisible_char(Some('•'));
    }
    entry
}

#[derive(Clone)]
enum McpFieldControl {
    Text {
        id: String,
        required: bool,
        entry: gtk::Entry,
    },
    Number {
        id: String,
        required: bool,
        integer: bool,
        minimum: Option<f64>,
        maximum: Option<f64>,
        entry: gtk::Entry,
    },
    Boolean {
        id: String,
        button: gtk::CheckButton,
    },
    Select {
        id: String,
        required: bool,
        multiple: bool,
        buttons: Vec<(gtk::CheckButton, String)>,
    },
}

impl McpFieldControl {
    fn value(&self) -> Result<Option<(String, serde_json::Value)>, ()> {
        match self {
            Self::Text {
                id,
                required,
                entry,
            } => {
                let value = entry.text().trim().to_owned();
                if value.is_empty() {
                    return if *required { Err(()) } else { Ok(None) };
                }
                Ok(Some((id.clone(), serde_json::Value::String(value))))
            }
            Self::Number {
                id,
                required,
                integer,
                minimum,
                maximum,
                entry,
            } => {
                let text = entry.text();
                let text = text.trim();
                if text.is_empty() {
                    return if *required { Err(()) } else { Ok(None) };
                }
                let parsed = text.parse::<f64>().map_err(|_| ())?;
                if minimum.is_some_and(|minimum| parsed < minimum)
                    || maximum.is_some_and(|maximum| parsed > maximum)
                {
                    return Err(());
                }
                let value = if *integer {
                    serde_json::Value::Number(text.parse::<i64>().map_err(|_| ())?.into())
                } else {
                    serde_json::Number::from_f64(parsed)
                        .map(serde_json::Value::Number)
                        .ok_or(())?
                };
                Ok(Some((id.clone(), value)))
            }
            Self::Boolean { id, button } => Ok(Some((
                id.clone(),
                serde_json::Value::Bool(button.is_active()),
            ))),
            Self::Select {
                id,
                required,
                multiple,
                buttons,
            } => {
                let selected = buttons
                    .iter()
                    .filter(|(button, _)| button.is_active())
                    .map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    return if *required { Err(()) } else { Ok(None) };
                }
                let value = if *multiple {
                    serde_json::Value::Array(
                        selected
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                    )
                } else {
                    serde_json::Value::String(selected[0].clone())
                };
                Ok(Some((id.clone(), value)))
            }
        }
    }

    fn connect_changed(&self, callback: Rc<dyn Fn()>) {
        match self {
            Self::Text { entry, .. } | Self::Number { entry, .. } => {
                entry.connect_changed(move |_| callback());
            }
            Self::Boolean { button, .. } => {
                button.connect_toggled(move |_| callback());
            }
            Self::Select { buttons, .. } => {
                for (button, _) in buttons {
                    button.connect_toggled({
                        let callback = callback.clone();
                        move |_| callback()
                    });
                }
            }
        }
    }
}

fn append_mcp_form_request(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request_id: &str,
    form: &McpFormRequest,
) {
    let controls = form
        .fields
        .iter()
        .map(|field| append_mcp_form_field(content, field))
        .collect::<Rc<Vec<_>>>();
    let submit = gtk::Button::with_label("Submit");
    submit.add_css_class("suggested-action");
    let update_submit: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let submit = submit.clone();
        Rc::new(move || {
            submit.set_sensitive(controls.iter().all(|control| control.value().is_ok()))
        })
    };
    for control in controls.iter() {
        control.connect_changed(update_submit.clone());
    }
    update_submit();

    submit.connect_clicked({
        let callbacks = callbacks.clone();
        let content = content.downgrade();
        let controls = controls.clone();
        let request_id = request_id.to_owned();
        move |_| {
            let values = controls
                .iter()
                .map(McpFieldControl::value)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .map(|values| values.into_iter().flatten().collect::<BTreeMap<_, _>>());
            let (Some(content), Some(content_values)) = (content.upgrade(), values) else {
                return;
            };
            resolve_structured_request(
                &content,
                &callbacks,
                &request_id,
                StructuredRequestResponse::McpElicitation {
                    action: McpElicitationResponseAction::Accept,
                    content: Some(content_values),
                },
            );
        }
    });
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    for (label, action, destructive) in [
        ("Decline", McpElicitationResponseAction::Decline, false),
        ("Cancel", McpElicitationResponseAction::Cancel, true),
    ] {
        let button = gtk::Button::with_label(label);
        if destructive {
            button.add_css_class("destructive-action");
        }
        button.connect_clicked({
            let callbacks = callbacks.clone();
            let content = content.downgrade();
            let request_id = request_id.to_owned();
            move |_| {
                let Some(content) = content.upgrade() else {
                    return;
                };
                resolve_structured_request(
                    &content,
                    &callbacks,
                    &request_id,
                    StructuredRequestResponse::McpElicitation {
                        action,
                        content: None,
                    },
                );
            }
        });
        actions.append(&button);
    }
    actions.append(&submit);
    content.append(&actions);
}

fn append_mcp_form_field(content: &gtk::Box, field: &McpFormField) -> McpFieldControl {
    let field_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    field_box.append(
        &gtk::Label::builder()
            .label(if field.required {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            })
            .css_classes(["heading"])
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    if let Some(description) = field.description.as_deref().filter(|value| !value.is_empty()) {
        field_box.append(
            &gtk::Label::builder()
                .label(description)
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build(),
        );
    }
    let control = match &field.kind {
        McpFormFieldKind::Text {
            default,
            placeholder,
            format,
            secret,
        } => {
            let entry = request_text_entry(*secret, placeholder.as_deref().or(format.as_deref()));
            if let Some(default) = default {
                entry.set_text(default);
            }
            field_box.append(&entry);
            McpFieldControl::Text {
                id: field.id.clone(),
                required: field.required,
                entry,
            }
        }
        McpFormFieldKind::Number {
            default,
            minimum,
            maximum,
            integer,
        } => {
            let range = match (minimum, maximum) {
                (Some(minimum), Some(maximum)) => Some(format!("{minimum} to {maximum}")),
                (Some(minimum), None) => Some(format!("At least {minimum}")),
                (None, Some(maximum)) => Some(format!("At most {maximum}")),
                (None, None) => None,
            };
            let entry = gtk::Entry::builder()
                .placeholder_text(range.as_deref().unwrap_or(if *integer {
                    "Enter an integer"
                } else {
                    "Enter a number"
                }))
                .input_purpose(if *integer {
                    gtk::InputPurpose::Digits
                } else {
                    gtk::InputPurpose::Number
                })
                .hexpand(true)
                .build();
            if let Some(default) = default {
                entry.set_text(default);
            }
            field_box.append(&entry);
            McpFieldControl::Number {
                id: field.id.clone(),
                required: field.required,
                integer: *integer,
                minimum: minimum.as_deref().and_then(|value| value.parse().ok()),
                maximum: maximum.as_deref().and_then(|value| value.parse().ok()),
                entry,
            }
        }
        McpFormFieldKind::Boolean { default } => {
            let button = gtk::CheckButton::with_label("Enabled");
            button.set_active(*default);
            field_box.append(&button);
            McpFieldControl::Boolean {
                id: field.id.clone(),
                button,
            }
        }
        McpFormFieldKind::Select {
            options,
            multiple,
            defaults,
        } => {
            let options_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .build();
            let mut group = None::<gtk::CheckButton>;
            let mut buttons = Vec::new();
            for option in options {
                let button = append_choice(&options_box, option);
                if !multiple {
                    if let Some(group) = group.as_ref() {
                        button.set_group(Some(group));
                    } else {
                        group = Some(button.clone());
                    }
                }
                button.set_active(defaults.contains(&option.value));
                buttons.push((button, option.value.clone()));
            }
            field_box.append(&options_box);
            McpFieldControl::Select {
                id: field.id.clone(),
                required: field.required,
                multiple: *multiple,
                buttons,
            }
        }
    };
    content.append(&field_box);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    control
}

fn append_mcp_url_request(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request_id: &str,
    request: &McpUrlRequest,
) {
    let link = gtk::LinkButton::with_label(&request.url, "Open requested page");
    link.set_tooltip_text(Some(&request.url));
    link.set_halign(gtk::Align::Start);
    content.append(&link);
    content.append(
        &gtk::Label::builder()
            .label(&request.url)
            .css_classes(["caption", "dim-label"])
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build(),
    );
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    for (label, action, css_class) in [
        (
            "Accept",
            McpElicitationResponseAction::Accept,
            Some("suggested-action"),
        ),
        ("Decline", McpElicitationResponseAction::Decline, None),
        (
            "Cancel",
            McpElicitationResponseAction::Cancel,
            Some("destructive-action"),
        ),
    ] {
        let button = gtk::Button::with_label(label);
        if let Some(css_class) = css_class {
            button.add_css_class(css_class);
        }
        button.connect_clicked({
            let callbacks = callbacks.clone();
            let content = content.downgrade();
            let request_id = request_id.to_owned();
            move |_| {
                let Some(content) = content.upgrade() else {
                    return;
                };
                resolve_structured_request(
                    &content,
                    &callbacks,
                    &request_id,
                    StructuredRequestResponse::McpElicitation {
                        action,
                        content: None,
                    },
                );
            }
        });
        actions.append(&button);
    }
    content.append(&actions);
}

fn append_dynamic_tool_request(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request_id: &str,
    request: &DynamicToolRequest,
) {
    let output = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(true)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .monospace(true)
        .build();
    output.set_size_request(-1, 112);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&output)
        .build();
    content.append(&scroller);
    if let Some(placeholder) = request
        .output_placeholder
        .as_deref()
        .filter(|placeholder| !placeholder.is_empty())
    {
        content.append(
            &gtk::Label::builder()
                .label(placeholder)
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
    }
    let submit = gtk::Button::with_label("Return output");
    submit.add_css_class("suggested-action");
    submit.set_sensitive(false);
    output.buffer().connect_changed({
        let submit = submit.clone();
        move |buffer| {
            submit.set_sensitive(
                !buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), true)
                    .trim()
                    .is_empty(),
            )
        }
    });
    submit.connect_clicked({
        let callbacks = callbacks.clone();
        let content = content.downgrade();
        let output = output.clone();
        let request_id = request_id.to_owned();
        move |_| {
            let buffer = output.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .trim()
                .to_owned();
            let Some(content) = content.upgrade() else {
                return;
            };
            if text.is_empty() {
                return;
            }
            resolve_structured_request(
                &content,
                &callbacks,
                &request_id,
                StructuredRequestResponse::DynamicTool {
                    content_items: vec![DynamicToolOutputContent::InputText { text }],
                    success: true,
                },
            );
        }
    });
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    if request.allows_failure {
        let fail = gtk::Button::with_label("Report failure");
        fail.add_css_class("destructive-action");
        fail.connect_clicked({
            let callbacks = callbacks.clone();
            let content = content.downgrade();
            let request_id = request_id.to_owned();
            move |_| {
                let Some(content) = content.upgrade() else {
                    return;
                };
                resolve_structured_request(
                    &content,
                    &callbacks,
                    &request_id,
                    StructuredRequestResponse::DynamicTool {
                        content_items: Vec::new(),
                        success: false,
                    },
                );
            }
        });
        actions.append(&fail);
    }
    actions.append(&submit);
    content.append(&actions);
}

fn resolve_structured_request(
    content: &gtk::Box,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
    request_id: &str,
    response: StructuredRequestResponse,
) {
    content.set_sensitive(false);
    emit_action(
        callbacks,
        CodexChatAction::ResolveRequest {
            request_id: request_id.to_owned(),
            response: PendingRequestResponse::structured(response),
        },
    );
}

fn selector_controls() -> (gtk::Box, HashMap<ChatSelector, SelectorControl>) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(4)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    let mut controls = HashMap::new();
    for selector in [
        ChatSelector::Model,
        ChatSelector::Reasoning,
        ChatSelector::Personality,
        ChatSelector::Permissions,
        ChatSelector::Collaboration,
    ] {
        let label = match selector {
            ChatSelector::Model => "Model",
            ChatSelector::Reasoning => "Reasoning",
            ChatSelector::Personality => "Personality",
            ChatSelector::Permissions => "Permissions",
            ChatSelector::Collaboration => "Collaboration",
        };
        let dropdown = gtk::DropDown::builder()
            .enable_search(true)
            .sensitive(false)
            .build();
        let control_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        control_box.append(
            &gtk::Label::builder()
                .label(label)
                .css_classes(["caption", "dim-label"])
                .xalign(0.0)
                .build(),
        );
        control_box.append(&dropdown);
        row.append(&control_box);
        controls.insert(
            selector,
            SelectorControl {
                dropdown,
                ids: RefCell::new(Vec::new()),
                updating: Cell::new(false),
            },
        );
    }
    (row, controls)
}

fn connect_selector_controls(state: &Rc<CodexChatViewState>) {
    for selector in [
        ChatSelector::Model,
        ChatSelector::Reasoning,
        ChatSelector::Personality,
        ChatSelector::Permissions,
        ChatSelector::Collaboration,
    ] {
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

fn connect_composer(
    state: &Rc<CodexChatViewState>,
    attach_button: &gtk::Button,
    mention_button: &gtk::Button,
    drop_widget: &gtk::Frame,
) {
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
    attach_button.connect_clicked({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                emit_action(&state.callbacks, CodexChatAction::ChooseAttachment);
            }
        }
    });
    mention_button.connect_clicked({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                emit_action(&state.callbacks, CodexChatAction::ChooseMention);
            }
        }
    });

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let state = Rc::downgrade(state);
        move |_, key, _, modifiers| {
            if key != gdk::Key::Return || !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let Some(state) = state.upgrade() else {
                return glib::Propagation::Proceed;
            };
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
    if steer != state.turn_active.get() {
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
    state
        .send_button
        .set_sensitive(can_submit && !state.turn_active.get());
    state
        .steer_button
        .set_sensitive(can_submit && state.turn_active.get());
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
        let icon_name = match attachment.kind {
            ComposerAttachmentKind::File => "text-x-generic-symbolic",
            ComposerAttachmentKind::Image => "image-x-generic-symbolic",
            ComposerAttachmentKind::Audio => "audio-x-generic-symbolic",
            ComposerAttachmentKind::Mention => "document-open-symbolic",
            ComposerAttachmentKind::Other => "mail-attachment-symbolic",
        };
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(14);
        let label = gtk::Label::builder()
            .label(&attachment.label)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(32)
            .build();
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove attachment")
            .build();
        remove.add_css_class("flat");
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
        let chip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .tooltip_text(&attachment.reference)
            .build();
        chip.add_css_class("card");
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
