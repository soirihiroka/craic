use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use craic_codex_app_server::protocol::RequestId;
use craic_codex_app_server::{AppServerError, ConnectionState};
use gtk::gio;
use serde_json::{Value, json};

use super::super::thread_picker::{ThreadPickerAction, ThreadPickerRow, ThreadPickerSort};
use super::notifications::timeline_from_item;
use super::{AppChatSessionInner, AppChatState, title_case};
use crate::ui::agent_history::{self, CodexThreadOverlay, CodexThreadOverlayUpsert};

pub(super) struct PickerRequest {
    query: String,
    append: bool,
    archived: bool,
    sort: ThreadPickerSort,
    cursor: Option<String>,
    generation: u64,
}

impl AppChatSessionInner {
    pub(super) fn connect_picker_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.picker.connect_action(move |action| {
            if let Some(session) = weak.upgrade() {
                session.handle_picker_action(action);
            }
        });
    }

    fn handle_picker_action(self: &Rc<Self>, action: ThreadPickerAction) {
        match action {
            ThreadPickerAction::SearchChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::ArchivedChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::SortChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::LoadMore => self.load_thread_page(true),
            ThreadPickerAction::Resume(thread_id) => {
                self.prepare_thread_switch();
                self.send_thread_operation(
                    "thread/resume",
                    &thread_id,
                    json!({
                        "excludeTurns": true,
                        "initialTurnsPage": {
                            "limit": 100,
                            "sortDirection": "desc",
                            "itemsView": "full"
                        }
                    }),
                );
            }
            ThreadPickerAction::Fork(thread_id) => {
                self.prepare_thread_switch();
                self.send_thread_operation("thread/fork", &thread_id, json!({}));
            }
            ThreadPickerAction::Rename {
                thread_id,
                current_name,
            } => self.prompt_thread_rename(thread_id, current_name),
            ThreadPickerAction::EditTags { thread_id, tags } => {
                self.prompt_thread_tags(thread_id, tags)
            }
            ThreadPickerAction::Archive(thread_id) => {
                self.send_thread_operation("thread/archive", &thread_id, json!({}));
            }
            ThreadPickerAction::Unarchive(thread_id) => {
                self.send_thread_operation("thread/unarchive", &thread_id, json!({}));
            }
            ThreadPickerAction::Delete(thread_id) => self.confirm_thread_delete(thread_id),
            ThreadPickerAction::Pin(thread_id) => self.send_thread_operation(
                "thread/metadata/update",
                &thread_id,
                json!({ "isPinned": true }),
            ),
            ThreadPickerAction::Unpin(thread_id) => self.send_thread_operation(
                "thread/metadata/update",
                &thread_id,
                json!({ "isPinned": false }),
            ),
            ThreadPickerAction::Cancel => {
                if self.thread_id.borrow().is_some() {
                    self.hide_thread_picker();
                }
            }
        }
    }

    fn prompt_thread_rename(self: &Rc<Self>, thread_id: String, current_name: String) {
        let dialog = adw::AlertDialog::builder()
            .heading("Rename Codex Thread")
            .body("Choose the name shown in Codex thread history.")
            .build();
        let entry = gtk::Entry::builder()
            .text(&current_name)
            .placeholder_text("Thread name")
            .activates_default(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "rename" {
                    return;
                }
                let name = entry.text().trim().to_owned();
                if name.is_empty() {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.send_thread_operation(
                        "thread/name/set",
                        &thread_id,
                        json!({ "name": name }),
                    );
                }
            },
        );
    }

    fn prompt_thread_tags(self: &Rc<Self>, thread_id: String, tags: Vec<String>) {
        let dialog = adw::AlertDialog::builder()
            .heading("Edit Thread Tags")
            .body("Separate tags with commas. Tags are stored locally for this workspace.")
            .build();
        let entry = gtk::Entry::builder()
            .text(tags.join(", "))
            .placeholder_text("bug, frontend, follow-up")
            .activates_default(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "save" {
                    return;
                }
                let tags = entry
                    .text()
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let Some(session) = weak.upgrade() else {
                    return;
                };
                match agent_history::update_codex_thread_overlay_tags(
                    &session.workspace_key,
                    &thread_id,
                    &tags,
                ) {
                    Ok(_) => {
                        session.load_thread_page(false);
                        if let Some(callback) = session.history_callback.borrow().clone() {
                            callback(session.id);
                        }
                    }
                    Err(error) => session.picker.set_error(Some(&error)),
                }
            },
        );
    }

    fn confirm_thread_delete(self: &Rc<Self>, thread_id: String) {
        let dialog = adw::AlertDialog::builder()
            .heading("Delete Codex Thread?")
            .body("This permanently deletes the thread and its local history metadata.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete Thread");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "delete" {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.send_thread_operation("thread/delete", &thread_id, json!({}));
                }
            },
        );
    }

    pub(super) fn show_thread_picker(&self) {
        self.content.set_visible_child_name("threads");
        self.picker.set_query("");
        self.picker.focus_search();
        self.load_thread_page(false);
    }

    pub(super) fn hide_thread_picker(&self) {
        self.content.set_visible_child_name("chat");
        self.view.focus_composer();
    }

    pub(super) fn load_thread_page(&self, append: bool) {
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            if matches!(
                *self.lifecycle.borrow(),
                AppChatState::Connecting
                    | AppChatState::Initializing
                    | AppChatState::StartingThread
            ) {
                self.picker.set_loading(true);
            } else {
                self.picker
                    .set_error(Some("Codex App Server is not connected."));
            }
            return;
        };
        if !append {
            self.picker_cursor.borrow_mut().take();
        }
        let query = self.picker.query();
        let cursor = self.picker_cursor.borrow().clone();
        let archived = self.picker.archived_only();
        let sort = self.picker.sort();
        let sort_key = match sort {
            ThreadPickerSort::Updated => "updated_at",
            ThreadPickerSort::Created => "created_at",
        };
        let generation = self.picker_generation.get().wrapping_add(1);
        self.picker_generation.set(generation);
        self.picker.set_loading(true);
        let params = json!({
            "cursor": cursor,
            "limit": 50,
            "sortKey": sort_key,
            "sortDirection": "desc",
            "cwd": self.workspace_root,
            "searchTerm": (!query.is_empty()).then_some(query.clone()),
            "archived": archived
        });
        match server.send_raw_request("thread/list", Some(params)) {
            Ok(request_id) => {
                self.picker_requests.borrow_mut().insert(
                    request_id,
                    PickerRequest {
                        query,
                        append,
                        archived,
                        sort,
                        cursor,
                        generation,
                    },
                );
            }
            Err(AppServerError::NotReady(
                ConnectionState::Starting | ConnectionState::Initializing,
            )) => self.picker.set_loading(true),
            Err(error) => self.picker.set_error(Some(&error.to_string())),
        }
    }

    pub(super) fn apply_thread_list(&self, request_id: &RequestId, result: &Value) {
        let Some(request) = self.picker_requests.borrow_mut().remove(request_id) else {
            return;
        };
        if request.generation != self.picker_generation.get()
            || self.picker.query() != request.query
            || self.picker.archived_only() != request.archived
            || self.picker.sort() != request.sort
            || *self.picker_cursor.borrow() != request.cursor
        {
            return;
        }
        let overlays =
            match agent_history::list_codex_thread_overlays(&self.workspace_key, 10_000, 0) {
                Ok(overlays) => overlays
                    .into_iter()
                    .map(|overlay| (overlay.thread_id.clone(), overlay))
                    .collect::<HashMap<_, _>>(),
                Err(error) => {
                    log::warn!(
                        "failed loading Codex thread overlays session_id={}: {error}",
                        self.id
                    );
                    HashMap::new()
                }
            };
        let rows = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|thread| self.thread_picker_row(thread, &overlays, request.archived))
            .collect::<Vec<_>>();
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let has_more = next_cursor.is_some();
        self.picker_cursor.replace(next_cursor);
        if request.append {
            self.picker.append_rows(rows, has_more);
        } else {
            self.picker.set_rows(rows, has_more);
        }
    }

    fn thread_picker_row(
        &self,
        thread: &Value,
        overlays: &HashMap<String, CodexThreadOverlay>,
        archived: bool,
    ) -> Option<ThreadPickerRow> {
        let thread_id = thread.get("id")?.as_str()?.to_owned();
        let overlay = overlays.get(&thread_id);
        let preview = thread
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let title = thread
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| overlay.and_then(|overlay| overlay.task_description.clone()))
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| preview.clone());
        Some(ThreadPickerRow {
            thread_id,
            title,
            preview,
            model: thread
                .get("model")
                .or_else(|| thread.get("modelProvider"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            updated_at_ms: thread
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .saturating_mul(1_000),
            status: thread
                .pointer("/status/type")
                .and_then(Value::as_str)
                .map(title_case),
            tags: overlay
                .map(|overlay| overlay.tags.clone())
                .unwrap_or_default(),
            archived,
            pinned: thread
                .get("isPinned")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub(super) fn send_thread_operation(&self, method: &str, thread_id: &str, extra: Value) {
        let mut params = extra.as_object().cloned().unwrap_or_default();
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        match server.send_raw_request(method, Some(Value::Object(params))) {
            Ok(request_id) => {
                self.thread_operations
                    .borrow_mut()
                    .insert(request_id, (method.to_owned(), thread_id.to_owned()));
            }
            Err(error) => {
                self.push_error(error.to_string());
                if matches!(method, "thread/resume" | "thread/fork") {
                    self.clear_local_history_identity();
                    self.set_state(AppChatState::Ready);
                    self.content.set_visible_child_name("threads");
                    self.picker.set_loading(false);
                }
                self.picker.set_error(Some(&error.to_string()));
            }
        }
    }

    pub(super) fn prepare_thread_switch(&self) {
        if self.active_turn_id.borrow().is_some() {
            self.interrupt();
        }
        if let Some(thread_id) = self.thread_id.borrow_mut().take()
            && let Some(server) = self.server.borrow().as_ref()
        {
            let _ = server
                .send_raw_request("thread/unsubscribe", Some(json!({ "threadId": thread_id })));
        }
        self.active_turn_id.borrow_mut().take();
        self.set_state(AppChatState::StartingThread);
        self.timeline.borrow_mut().clear();
        self.clear_pending_requests();
        self.tool_requests.borrow_mut().clear();
        for queued in self.queued_turns.replace(Vec::new()) {
            for attachment in queued.submission.attachments {
                self.remove_temporary_attachment(&attachment.id);
            }
        }
        self.sync_queued_submissions();
        self.view.clear_timeline();
        self.view.set_usage(None);
        self.turns_cursor.borrow_mut().take();
        self.turns_request.borrow_mut().take();
        self.view.set_older_turns_available(false);
        self.view.set_older_turns_loading(false);
        self.view.set_turn_active(false);
        self.view.set_composer_enabled(false);
        self.set_title("New Codex chat");
        self.clear_local_history_identity();
        self.picker.set_loading(true);
    }

    pub(super) fn load_thread_history(&self, result: &Value) {
        self.timeline.borrow_mut().clear();
        self.view.clear_timeline();
        let initial_page = result.get("initialTurnsPage");
        let turns = initial_page
            .and_then(|page| page.get("data"))
            .and_then(Value::as_array)
            .or_else(|| result.pointer("/thread/turns").and_then(Value::as_array));
        let mut turns = turns.cloned().unwrap_or_default();
        if initial_page.is_some() {
            turns.reverse();
        }
        let next_cursor = initial_page
            .and_then(|page| page.get("nextCursor"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.turns_cursor.replace(next_cursor.clone());
        self.view.set_older_turns_available(next_cursor.is_some());
        self.view.set_older_turns_loading(false);
        for turn in &turns {
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.upsert_timeline(timeline_from_item(item, true));
            }
            if turn.get("status").and_then(Value::as_str) == Some("inProgress")
                && let Some(turn_id) = turn.get("id").and_then(Value::as_str)
            {
                self.active_turn_id.replace(Some(turn_id.to_owned()));
                self.view.set_turn_active(true);
            }
        }
    }

    pub(super) fn load_older_turns(&self) {
        if self.turns_request.borrow().is_some() {
            return;
        }
        let (Some(thread_id), Some(cursor)) = (
            self.thread_id.borrow().clone(),
            self.turns_cursor.borrow().clone(),
        ) else {
            self.view.set_older_turns_available(false);
            return;
        };
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        self.view.set_older_turns_loading(true);
        match server.send_raw_request(
            "thread/turns/list",
            Some(json!({
                "threadId": thread_id,
                "cursor": cursor,
                "limit": 100,
                "sortDirection": "desc",
                "itemsView": "full"
            })),
        ) {
            Ok(request_id) => {
                self.turns_request.replace(Some(request_id));
            }
            Err(error) => {
                self.view.set_older_turns_loading(false);
                self.push_error(error.to_string());
            }
        }
    }

    pub(super) fn apply_older_turns(&self, request_id: &RequestId, result: &Value) {
        if self.turns_request.borrow().as_ref() != Some(request_id) {
            return;
        }
        self.turns_request.borrow_mut().take();
        let mut turns = result
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        turns.reverse();
        let mut items = Vec::new();
        for turn in &turns {
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let item = timeline_from_item(item, true);
                if !self.timeline.borrow().contains_key(&item.id) {
                    self.timeline
                        .borrow_mut()
                        .insert(item.id.clone(), item.clone());
                    items.push(item);
                }
            }
        }
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.turns_cursor.replace(next_cursor.clone());
        self.view.prepend_timeline_items(&items);
        self.view.set_older_turns_loading(false);
        self.view.set_older_turns_available(next_cursor.is_some());
    }

    pub(super) fn thread_command(&self, method: &str, extra: Value) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            return;
        };
        let mut params = extra.as_object().cloned().unwrap_or_default();
        params.insert("threadId".to_owned(), Value::String(thread_id));
        if let Some(server) = self.server.borrow().as_ref()
            && let Err(error) = server.send_raw_request(method, Some(Value::Object(params)))
        {
            self.push_error(error.to_string());
        }
    }

    pub(super) fn handle_history_error(
        &self,
        request_id: &RequestId,
        method: Option<&str>,
        message: &str,
    ) {
        self.thread_operations.borrow_mut().remove(request_id);
        if method == Some("thread/turns/list")
            && self.turns_request.borrow().as_ref() == Some(request_id)
        {
            self.turns_request.borrow_mut().take();
            self.view.set_older_turns_loading(false);
        }
        if method == Some("thread/list") {
            let request = self.picker_requests.borrow_mut().remove(request_id);
            if request.is_some_and(|request| {
                request.generation == self.picker_generation.get()
                    && request.query == self.picker.query()
                    && request.archived == self.picker.archived_only()
                    && request.sort == self.picker.sort()
                    && request.cursor == *self.picker_cursor.borrow()
            }) {
                self.picker.set_error(Some(message));
            }
        }
        if matches!(method, Some("thread/resume" | "thread/fork")) {
            self.clear_local_history_identity();
            self.set_state(AppChatState::Ready);
            self.content.set_visible_child_name("threads");
            self.picker.set_loading(false);
            self.picker.set_error(Some(message));
        }
    }

    pub(super) fn apply_thread_operation_response(&self, request_id: &RequestId) {
        if let Some((operation, thread_id)) = self.thread_operations.borrow_mut().remove(request_id)
            && operation == "thread/delete"
            && let Err(error) =
                agent_history::delete_codex_thread_overlay(&self.workspace_key, &thread_id)
        {
            log::warn!(
                "failed deleting Codex thread overlay session_id={} thread_id={}: {error}",
                self.id,
                thread_id
            );
        }
        self.load_thread_page(false);
    }

    pub(super) fn refresh_picker_for_notification(&self, method: &str) {
        if matches!(
            method,
            "thread/name/updated"
                | "thread/archived"
                | "thread/unarchived"
                | "thread/deleted"
                | "thread/started"
        ) && self.content.visible_child_name().as_deref() == Some("threads")
        {
            self.load_thread_page(false);
        }
    }

    pub(super) fn persist_overlay(&self, task_description: Option<String>) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            return;
        };
        if let Some(callback) = self.thread_callback.borrow().clone() {
            callback(self.id, thread_id.clone(), self.title.borrow().clone());
        }
        let existing = match agent_history::lookup_codex_thread_overlay(
            &self.workspace_key,
            &thread_id,
        ) {
            Ok(existing) => existing,
            Err(error) => {
                log::warn!(
                    "failed reading Codex thread overlay before update session_id={} thread_id={}: {error}",
                    self.id,
                    thread_id
                );
                return;
            }
        };
        let task_description = task_description
            .or_else(|| {
                (self.title.borrow().as_str() != "New Codex chat")
                    .then(|| self.title.borrow().clone())
            })
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|overlay| overlay.task_description.clone())
            });
        let tags = existing.map(|overlay| overlay.tags).unwrap_or_default();
        match agent_history::upsert_codex_thread_overlay(CodexThreadOverlayUpsert {
            thread_id,
            workspace_key: self.workspace_key.clone(),
            task_description,
            tags,
        }) {
            Ok(_) => {
                if let Some(callback) = self.history_callback.borrow().clone() {
                    callback(self.id);
                }
            }
            Err(error) => log::warn!(
                "failed to persist Codex thread overlay session_id={}: {error}",
                self.id
            ),
        }
    }

    fn clear_local_history_identity(&self) {
        let Some(local_id) = self.local_history_id.take() else {
            return;
        };
        if let Err(error) = agent_history::mark_ended(local_id) {
            log::warn!(
                "failed marking switched Codex App session ended session_id={} local_id={local_id}: {error}",
                self.id
            );
        }
        if let Some(callback) = self.thread_callback.borrow().clone() {
            callback(self.id, String::new(), self.title.borrow().clone());
        }
        if let Some(callback) = self.history_callback.borrow().clone() {
            callback(self.id);
        }
    }
}
