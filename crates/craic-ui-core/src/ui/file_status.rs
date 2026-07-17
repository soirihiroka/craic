use adw::prelude::*;

pub fn icon(status: &str) -> gtk::Image {
    let kind = StatusKind::from_status(status);
    let image = gtk::Image::builder()
        .icon_name(kind.icon_name())
        .pixel_size(14)
        .tooltip_text(kind.tooltip())
        .valign(gtk::Align::Center)
        .build();
    image.add_css_class(kind.color_class());
    image
}

enum StatusKind {
    Added,
    Deleted,
    Renamed,
    Conflicted,
    Modified,
}

impl StatusKind {
    fn from_status(status: &str) -> Self {
        if status.contains('U') {
            Self::Conflicted
        } else if status.contains('D') {
            Self::Deleted
        } else if status.contains('A') || status.contains('?') {
            Self::Added
        } else if status.contains('R') {
            Self::Renamed
        } else {
            Self::Modified
        }
    }

    fn icon_name(&self) -> &'static str {
        match self {
            Self::Added => "list-add-symbolic",
            Self::Deleted => "edit-delete-symbolic",
            Self::Renamed => "document-open-recent-symbolic",
            Self::Conflicted => "dialog-warning-symbolic",
            Self::Modified => "document-edit-symbolic",
        }
    }

    fn tooltip(&self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Conflicted => "Conflicted",
            Self::Modified => "Modified",
        }
    }

    fn color_class(&self) -> &'static str {
        match self {
            Self::Added => "success",
            Self::Deleted | Self::Conflicted => "error",
            Self::Renamed => "accent",
            Self::Modified => "warning",
        }
    }
}
