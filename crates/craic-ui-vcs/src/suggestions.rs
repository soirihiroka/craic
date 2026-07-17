use adw::prelude::*;
use gtk::pango;

pub struct SuggestionsPanel {
    pub root: gtk::Box,
    pub actions: SuggestionsActions,
}

pub struct SuggestionsActions {
    pub open_editor: gtk::Button,
    pub open_terminal: gtk::Button,
    pub show_files: gtk::Button,
    pub view_github: gtk::Button,
    pub git_button: gtk::Button,
    pub git_card: gtk::Box,
    pub git_title: gtk::Label,
    pub git_subtitle: gtk::Label,
}

impl SuggestionsPanel {
    pub fn new() -> Self {
        let open_editor = gtk::Button::with_label("Open");
        let open_terminal = gtk::Button::with_label("Open");
        let show_files = gtk::Button::with_label("Show");
        let view_github = gtk::Button::with_label("View");
        let git_title = crate::ui::widgets::heading("");
        let git_subtitle = crate::ui::widgets::muted("");
        let git_button = gtk::Button::builder()
            .valign(gtk::Align::Center)
            .halign(gtk::Align::End)
            .build();
        git_button.add_css_class("suggested-action");

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();
        text.append(&git_title);
        text.append(&git_subtitle);

        let row = action_row(&text, &git_button);
        let git_card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .visible(false)
            .build();
        git_card.add_css_class("card");
        git_card.add_css_class("git-action-card");
        git_card.append(&row);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        root.append(&git_card);
        root.append(&action_card(
            "Open in editor",
            "Jump into the project files.",
            &open_editor,
        ));
        root.append(&action_card(
            "Open in Ghostty",
            "Open the repository in an external Ghostty window.",
            &open_terminal,
        ));
        root.append(&action_card(
            "Open in Files",
            "Open the repository folder in the external file manager.",
            &show_files,
        ));
        root.append(&action_card(
            "View on GitHub",
            "Open the remote repository.",
            &view_github,
        ));

        Self {
            root,
            actions: SuggestionsActions {
                open_editor,
                open_terminal,
                show_files,
                view_github,
                git_button,
                git_card,
                git_title,
                git_subtitle,
            },
        }
    }
}

fn action_card(title: &str, subtitle: &str, button: &gtk::Button) -> gtk::Box {
    let title = clipped_label(&crate::ui::widgets::heading(title));
    let subtitle = clipped_label(&crate::ui::widgets::muted(subtitle));
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    text.append(&title);
    text.append(&subtitle);

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    card.add_css_class("card");
    card.append(&action_row(&text, button));
    card
}

fn action_row(text: &gtk::Box, button: &gtk::Button) -> gtk::Box {
    button.set_valign(gtk::Align::Center);
    button.set_halign(gtk::Align::End);
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(20)
        .margin_end(20)
        .hexpand(true)
        .build();
    row.append(text);
    row.append(button);
    row
}

pub fn page(title: &gtk::Label, subtitle: &gtk::Label, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let heading = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    heading.append(title);
    heading.append(subtitle);

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(22)
        .build();
    page.append(&heading);
    page.append(body);
    page
}

pub fn centered_page(content: gtk::Box) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder()
        .maximum_size(640)
        .tightening_threshold(520)
        .child(&content)
        .build();
    let wrapper = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(32)
        .margin_end(32)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Start)
        .hexpand(true)
        .build();
    wrapper.append(&clamp);

    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&wrapper)
        .build()
}

fn clipped_label(label: &gtk::Label) -> gtk::Label {
    label.set_wrap(false);
    label.set_ellipsize(pango::EllipsizeMode::End);
    label.clone()
}
