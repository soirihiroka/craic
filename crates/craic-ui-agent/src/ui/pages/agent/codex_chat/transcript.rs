use super::view::ActionCallback;
use super::{
    CodexChatAction, DynamicToolOutputContent, DynamicToolRequest, McpElicitationResponseAction,
    McpFormField, McpFormFieldKind, McpFormRequest, McpUrlRequest, PendingRequest,
    PendingRequestKind, PendingRequestResponse, RequestOptionStyle, RequestSelectionMode,
    RequestUserInput, RequestUserInputAnswer, RequestUserInputQuestion, StructuredRequestOption,
    StructuredRequestResponse, TimelineItem, TimelineItemKind, TimelineItemStatus,
};
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub(super) fn timeline_row(item: &TimelineItem) -> gtk::Widget {
    let (default_title, icon_name) = match &item.kind {
        TimelineItemKind::UserMessage => ("You", "avatar-default-symbolic"),
        TimelineItemKind::AssistantMessage => ("Codex", "system-run-symbolic"),
        TimelineItemKind::DeveloperMessage => ("Developer message", "dialog-information-symbolic"),
        TimelineItemKind::Reasoning => ("Reasoning", "brain-augemnted-symbolic"),
        TimelineItemKind::Plan => ("Plan", "view-list-symbolic"),
        TimelineItemKind::Command => ("Command", "utilities-terminal-symbolic"),
        TimelineItemKind::FileChange => ("File change", "document-edit-symbolic"),
        TimelineItemKind::Tool => ("Tool", "applications-system-symbolic"),
        TimelineItemKind::McpTool => ("MCP tool", "network-server-symbolic"),
        TimelineItemKind::Web => ("Web", "web-browser-symbolic"),
        TimelineItemKind::Image => ("Image", "image-x-generic-symbolic"),
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
            TimelineItemKind::Command | TimelineItemKind::FileChange
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

pub(super) fn pending_request_row(
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
        PendingRequestKind::DynamicToolOutput(_) => "applications-system-symbolic",
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
                    let text = entry.text();
                    let answer = text.trim();
                    if answer.is_empty() {
                        return None;
                    }
                    answers.push(answer.to_owned());
                }
                (!answers.is_empty())
                    .then(|| (question_id.clone(), RequestUserInputAnswer { answers }))
            }
        }
    }

    fn connect_changed(&self, callback: Rc<dyn Fn()>) {
        match self {
            Self::Text { entry, .. } => {
                entry.connect_changed(move |_| callback());
            }
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
    let controls = Rc::new(
        input
            .questions
            .iter()
            .map(|question| append_user_input_question(content, question))
            .collect::<Vec<_>>(),
    );
    let submit = gtk::Button::with_label("Submit answers");
    submit.add_css_class("suggested-action");
    let update_submit: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let submit = submit.clone();
        Rc::new(move || {
            submit.set_sensitive(controls.iter().all(|control| control.answer().is_some()))
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

fn append_choice(container: &gtk::Box, option: &StructuredRequestOption) -> gtk::CheckButton {
    let button = gtk::CheckButton::with_label(&option.label);
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    row.append(&button);
    if let Some(description) = option
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
    {
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
        minimum_length: Option<u32>,
        maximum_length: Option<u32>,
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
        minimum_items: Option<u64>,
        maximum_items: Option<u64>,
        buttons: Vec<(gtk::CheckButton, String)>,
    },
}

impl McpFieldControl {
    fn value(&self) -> Result<Option<(String, serde_json::Value)>, ()> {
        match self {
            Self::Text {
                id,
                required,
                minimum_length,
                maximum_length,
                entry,
            } => {
                let value = entry.text().trim().to_owned();
                if value.is_empty() {
                    return if *required { Err(()) } else { Ok(None) };
                }
                let length = value.chars().count() as u32;
                if minimum_length.is_some_and(|minimum| length < minimum)
                    || maximum_length.is_some_and(|maximum| length > maximum)
                {
                    return Err(());
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
                minimum_items,
                maximum_items,
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
                let selected_count = selected.len() as u64;
                if minimum_items.is_some_and(|minimum| selected_count < minimum)
                    || maximum_items.is_some_and(|maximum| selected_count > maximum)
                {
                    return Err(());
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
    let controls = Rc::new(
        form.fields
            .iter()
            .map(|field| append_mcp_form_field(content, field))
            .collect::<Vec<_>>(),
    );
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
    if let Some(description) = field
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
    {
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
            minimum_length,
            maximum_length,
            secret,
        } => {
            let entry = request_text_entry(*secret, placeholder.as_deref().or(format.as_deref()));
            if !secret {
                entry.set_input_purpose(match format.as_deref() {
                    Some("email") => gtk::InputPurpose::Email,
                    Some("uri") => gtk::InputPurpose::Url,
                    _ => gtk::InputPurpose::FreeForm,
                });
            }
            if let Some(maximum_length) = maximum_length {
                entry.set_max_length((*maximum_length).min(i32::MAX as u32) as i32);
            }
            if let Some(default) = default {
                entry.set_text(default);
            }
            field_box.append(&entry);
            McpFieldControl::Text {
                id: field.id.clone(),
                required: field.required,
                minimum_length: *minimum_length,
                maximum_length: *maximum_length,
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
            button.set_active(default.unwrap_or(false));
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
            minimum_items,
            maximum_items,
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
                minimum_items: *minimum_items,
                maximum_items: *maximum_items,
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

fn emit_action(callbacks: &RefCell<Vec<ActionCallback>>, action: CodexChatAction) {
    for callback in callbacks.borrow().iter() {
        callback(action.clone());
    }
}
