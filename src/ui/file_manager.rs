use adw::prelude::*;

pub(super) fn app_launch_context(
    window: &adw::ApplicationWindow,
    event_time: u32,
) -> gtk::gdk::AppLaunchContext {
    let context = gtk::prelude::WidgetExt::display(window).app_launch_context();
    context.set_timestamp(if event_time == 0 {
        gtk::gdk::CURRENT_TIME
    } else {
        event_time
    });
    context
}
