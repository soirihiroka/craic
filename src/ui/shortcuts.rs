use adw::prelude::*;

const SHORTCUT_SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Application",
        &[
            ("Open New Window", "<Control>n"),
            ("Open Preferences", "<Control>comma"),
            ("Keyboard Shortcuts", "<Control>question"),
            ("About Craic", "F1"),
        ],
    ),
    (
        "Repository",
        &[
            ("Pull remote changes", "<Control>p"),
            ("Push local commits", "<Control>u"),
            ("Refresh repository status", "<Control>r"),
        ],
    ),
    (
        "File Browser",
        &[
            ("Copy selected file entries", "<Control>c"),
            ("Cut selected file entries", "<Control>x"),
            ("Paste file entries", "<Control>v"),
            ("Delete selected file entry", "Delete"),
            ("Select previous entry", "Up"),
            ("Select next entry", "Down"),
            ("Open / expand selected folder", "Return"),
            ("Cancel file rename/new-entry mode", "Escape"),
        ],
    ),
    (
        "Code Editor",
        &[
            ("Toggle search", "<Control>f"),
            ("Select all", "<Control>a"),
            ("Undo", "<Control>z"),
            ("Redo", "<Control><Shift>z"),
            ("Redo", "<Control>y"),
            ("Toggle line comment", "<Control>slash"),
            ("Copy", "<Control>c"),
            ("Copy", "<Control>Insert"),
            ("Cut", "<Control>x"),
            ("Paste", "<Control>v"),
            ("Toggle word wrap", "<Control>w"),
            ("Increase font size", "<Control>plus"),
            ("Increase font size", "<Control>equal"),
            ("Increase font size", "<Control>KP_Add"),
            ("Decrease font size", "<Control>minus"),
            ("Decrease font size", "<Control>underscore"),
            ("Decrease font size", "<Control>KP_Subtract"),
        ],
    ),
    (
        "Terminal",
        &[
            ("Copy terminal selection", "<Control>c"),
            ("Copy terminal selection", "<Control><Shift>C"),
            ("Copy terminal selection", "<Control>Insert"),
            ("Paste terminal selection", "<Control><Shift>v"),
            ("Paste terminal selection", "<Shift>Insert"),
            ("Delete previous word", "<Control>BackSpace"),
            ("Increase terminal font size", "<Control>plus"),
            ("Increase terminal font size", "<Control>equal"),
            ("Increase terminal font size", "<Control>KP_Add"),
            ("Decrease terminal font size", "<Control>minus"),
            ("Decrease terminal font size", "<Control>underscore"),
            ("Decrease terminal font size", "<Control>KP_Subtract"),
        ],
    ),
    (
        "PDF Preview",
        &[
            ("Zoom in", "<Control>plus"),
            ("Zoom in", "<Control>equal"),
            ("Zoom in", "<Control>KP_Add"),
            ("Zoom out", "<Control>minus"),
            ("Zoom out", "<Control>underscore"),
            ("Zoom out", "<Control>KP_Subtract"),
            ("Copy selected text", "<Control>c"),
        ],
    ),
];

pub(super) fn show_shortcuts_window(parent: &adw::ApplicationWindow) {
    let shortcuts_dialog = adw::ShortcutsDialog::builder()
        .title("Keyboard Shortcuts")
        .build();

    for &(section_title, shortcuts) in SHORTCUT_SECTIONS {
        let section = adw::ShortcutsSection::new(Some(section_title));

        for &(shortcut_title, accelerator) in shortcuts {
            section.add(adw::ShortcutsItem::new(shortcut_title, accelerator));
        }

        shortcuts_dialog.add(section);
    }

    log::debug!("Displaying keyboard shortcuts dialog");
    shortcuts_dialog.present(Some(parent));
}
