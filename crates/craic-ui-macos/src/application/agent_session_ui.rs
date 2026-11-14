impl AppDelegate {
    fn set_changes_operation_progress(&self, message: &str) {
        if let Some(check) = self.ivars().select_all_check.get() {
            check.setEnabled(false);
        }
        if let Some(label) = self.ivars().select_all_label.get() {
            label.setStringValue(&NSString::from_str(message));
        }
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_repository_available(false);
        }
    }

    fn show_native_toast(&self, message: &str) {
        let (Some(toast), Some(label)) = (self.ivars().toast.get(), self.ivars().toast_label.get())
        else {
            return;
        };
        if let Some(timer) = self.ivars().toast_timer.borrow_mut().take() {
            timer.invalidate();
        }
        label.setStringValue(&NSString::from_str(message));
        label.setToolTip(Some(&NSString::from_str(message)));
        toast.setAccessibilityLabel(Some(&NSString::from_str(message)));
        toast.setHidden(false);
        // SAFETY: The timer runs on AppKit's main run loop and targets a selector implemented by
        // this retained application delegate. Replacing a toast invalidates the previous timer.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                4.0,
                self.as_ref(),
                sel!(hideNativeToast:),
                None,
                false,
            )
        };
        self.ivars().toast_timer.replace(Some(timer));
        log::debug!("native toast presented message={message}");
    }

    fn changes_operation_failed(&self, heading: &str, message: &str) {
        self.refresh_selection_header();
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_repository_available(self.ivars().git_handle.borrow().is_some());
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(heading));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
        log::warn!("native changes operation failed heading={heading}: {message}");
    }

    fn active_agent_identity(&self) -> Option<AgentIdentity> {
        let agents = self.ivars().agents.get()?;
        Some(AgentIdentity {
            workspace_id: self.ivars().active_workspace_id.borrow().clone()?,
            generation: agents.generation.get(),
        })
    }

    fn apply_native_agent_event(&self, event: NativeAgentEvent) {
        match event {
            NativeAgentEvent::TerminalPrepared {
                identity,
                session_id,
                provider_id,
                history,
                result,
            } => {
                self.finish_native_terminal_agent_start(
                    &identity,
                    session_id,
                    &provider_id,
                    history,
                    result,
                );
            }
            NativeAgentEvent::HistoryLoaded {
                identity,
                request_id,
                result,
            } => {
                self.apply_native_cli_history(&identity, request_id, result);
            }
            NativeAgentEvent::SummaryFinished {
                identity,
                session_id,
                automatic,
                result,
            } => {
                if !self.agent_event_is_current(&identity) {
                    return;
                }
                if let Some(session) = self
                    .ivars()
                    .terminal_sessions
                    .borrow_mut()
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    session.agent_summary_in_flight = false;
                }
                match result {
                    Ok(()) => {
                        self.request_native_cli_history(false);
                        if !automatic {
                            self.show_native_toast("Session summary updated.");
                        }
                    }
                    Err(error) => {
                        log::warn!("native agent summary failed session={session_id}: {error}");
                        if !automatic {
                            self.present_path_action_error("Generate Summary Failed", &error);
                        }
                    }
                }
            }
            NativeAgentEvent::ProviderUpdatesChecked {
                identity,
                generation,
                result,
            } => {
                self.apply_native_provider_updates(&identity, generation, result);
            }
            NativeAgentEvent::ProviderUpdatePrepared {
                identity,
                generation,
                result,
            } => {
                self.finish_native_provider_update_start(&identity, generation, result);
            }
        }
    }

    fn agent_event_is_current(&self, identity: &AgentIdentity) -> bool {
        self.ivars().active_workspace_id.borrow().as_deref() == Some(identity.workspace_id.as_str())
            && self
                .ivars()
                .agents
                .get()
                .is_some_and(|agents| agents.generation.get() == identity.generation)
    }
}
