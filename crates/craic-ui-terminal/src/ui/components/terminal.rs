use gtk::gdk;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalActivation {
    Url(String),
    File(TerminalFileActivation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFileActivation {
    pub target: String,
    pub launch_dir: String,
}

pub fn modified_enter_sequence(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<String> {
    if !matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
        return None;
    }

    let mut mask = 1;
    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        mask += 1;
    }
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        mask += 2;
    }
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        mask += 4;
    }
    if modifiers.contains(gdk::ModifierType::SUPER_MASK) {
        mask += 8;
    }

    (mask != 1).then(|| format!("\x1b[13;{mask}:1u"))
}
