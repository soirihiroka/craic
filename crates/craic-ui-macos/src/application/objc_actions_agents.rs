impl AppDelegate {
    fn handle_objc_new_agent_chat(&self, _sender: &NSComboButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let provider = NATIVE_AGENT_PROVIDERS[agents.selected_provider.get()];
        self.start_native_terminal_agent(provider.provider_id(), provider.label(), None);
    }

    fn handle_objc_select_agent_provider(&self, sender: &NSMenuItem) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let Some((index, provider)) = usize::try_from(sender.tag()).ok().and_then(|index| {
            NATIVE_AGENT_PROVIDERS
                .get(index)
                .map(|provider| (index, *provider))
        }) else {
            return;
        };
        agents.selected_provider.set(index);
        let label = provider.label();
        agents.new_chat.setTitle(&NSString::from_str(label));
        agents.new_chat.setImage(
            agents
                .provider_icons
                .get(provider.provider_id())
                .map(|image| &**image),
        );
        for item in agents
            .new_chat
            .menu()
            .itemArray()
            .iter()
            .take(NATIVE_AGENT_PROVIDERS.len())
        {
            item.setState(if item.tag() == sender.tag() {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        agents
            .new_chat
            .setToolTip(Some(&NSString::from_str(&format!("New {label} chat"))));
    }

    fn handle_objc_resume_agent_thread(&self, sender: &NSButton) {
        self.resume_native_cli_history(sender.tag());
    }

    fn handle_objc_filter_agent_threads(&self, sender: &NSSearchField) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        agents
            .history_query
            .replace(sender.stringValue().to_string());
        self.request_native_cli_history(true);
    }

    fn handle_objc_select_agent_thread_scope(&self, sender: &NSPopUpButton) {
        let Some(agents) = self.ivars().agents.get() else {
            return;
        };
        let _ = sender;
        agents
            .history_limit
            .set(agents.history_limit.get().saturating_add(32));
        self.request_native_cli_history(false);
    }

    fn handle_objc_select_page(&self, sender: &NSToolbarItemGroup) {
        let Some((index, descriptor)) =
            usize::try_from(sender.selectedIndex())
                .ok()
                .and_then(|index| {
                    PAGE_DESCRIPTORS
                        .get(index)
                        .map(|descriptor| (index, descriptor))
                })
        else {
            return;
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            log::warn!("page activation ignored because application actor is unavailable");
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ActivatePage(descriptor.page_id())) {
            log::warn!("page activation queue rejected command={command:?}");
        } else {
            NSUserDefaults::standardUserDefaults()
                .setInteger_forKey(index as isize, &NSString::from_str(ACTIVE_PAGE_DEFAULT));
        }
    }
}
