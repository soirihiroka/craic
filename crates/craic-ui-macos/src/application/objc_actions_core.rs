impl AppDelegate {
    fn handle_objc_stop_application_run_loop(&self, _sender: Option<&AnyObject>) {
        log::info!("native AppKit main run loop stopping for controlled shutdown");
        let application = NSApplication::sharedApplication(self.mtm());
        application.stop(None);
        if let Some(wake_event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        ) {
            application.postEvent_atStart(&wake_event, true);
        }
    }

    fn handle_objc_finish_confirmed_close(&self, _sender: Option<&AnyObject>) {
        let quit_requested = self
            .ivars()
            .quit_requested_during_close_confirmation
            .replace(false);
        log::info!(
            "native confirmed close continuing after sheet dismissal quit_requested={quit_requested}"
        );
        if quit_requested {
            self.prepare_for_native_shutdown();
            self.schedule_controlled_app_stop();
        } else if let Some(window) = self.ivars().window.get() {
            window.performClose(None);
        }
    }
}
