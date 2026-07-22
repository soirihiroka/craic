use crate::gitignore;
use crate::gitignore::IgnoreTargetKind;
use crate::ui::components::context_menu::{self, ContextMenuBuilder};

use super::{BrowserTarget, ContainerFileAction};

pub fn repository_row_menu(
    target: &BrowserTarget,
    terminal_available: bool,
    container_actions_available: bool,
) -> ContextMenuBuilder {
    if target.is_dir {
        repository_folder_menu(target, terminal_available)
    } else {
        repository_file_menu(target, terminal_available, container_actions_available)
    }
}

fn repository_folder_menu(target: &BrowserTarget, terminal_available: bool) -> ContextMenuBuilder {
    let is_root = target.is_root();
    let mut menu = context_menu::builder("repo_file")
        .item("New File...", "new-file")
        .item("New Folder...", "new-folder")
        .separator()
        .item("Open", "open")
        .item("Open in File Manager", "open-external")
        .item("Open Containing Folder", "open-containing-folder");
    if terminal_available {
        menu = menu.item("Open in Integrated Terminal", "open-terminal");
        if target.executable {
            menu = menu.item("Run in Integrated Terminal", "run-terminal");
        }
    }
    menu = menu.separator();
    if !is_root {
        menu = menu
            .shortcut_item("Cut", "cut", "<Primary>x")
            .shortcut_item("Copy", "copy", "<Primary>c");
    }
    menu = menu.shortcut_item("Paste", "paste", "<Primary>v");

    if !is_root && target.capabilities.native {
        menu = append_ignore_section(menu.separator(), &target.path, IgnoreTargetKind::Folder);
    }

    menu = append_path_section(menu.separator(), !is_root);
    if !is_root {
        menu = append_destructive_section(menu.separator());
    }

    menu
}

fn repository_file_menu(
    target: &BrowserTarget,
    terminal_available: bool,
    container_actions_available: bool,
) -> ContextMenuBuilder {
    let mut menu = context_menu::builder("repo_file")
        .item("Open", "open")
        .item("Open With...", "open-external")
        .item("Open Containing Folder", "open-containing-folder");
    if terminal_available {
        menu = menu.item("Open in Integrated Terminal", "open-terminal");
    }
    menu = menu
        .item("Add File to Chat", "add-to-chat")
        .separator()
        .shortcut_item("Cut", "cut", "<Primary>x")
        .shortcut_item("Copy", "copy", "<Primary>c")
        .shortcut_item("Paste", "paste", "<Primary>v");

    if target.capabilities.native {
        menu = append_ignore_section(
            menu.separator(),
            target.path.as_str(),
            IgnoreTargetKind::File,
        );
    }

    if container_actions_available && target.capabilities.native {
        menu = append_container_section(menu.separator(), target);
    }

    menu = append_path_section(menu.separator(), true);
    menu = append_destructive_section(menu.separator());

    menu
}

fn append_container_section(
    mut menu: ContextMenuBuilder,
    target: &BrowserTarget,
) -> ContextMenuBuilder {
    let actions = target.container_actions();
    if actions.is_empty() {
        return menu;
    }

    for action in actions {
        let (label, action) = match action {
            ContainerFileAction::BuildImage => ("Build Image", "build-image"),
            ContainerFileAction::ComposeUp => ("Compose Up", "compose-up"),
            ContainerFileAction::ComposePull => ("Compose Pull", "compose-pull"),
            ContainerFileAction::ComposeRestart => ("Compose Restart", "compose-restart"),
            ContainerFileAction::ComposeDown => ("Compose Down", "compose-down"),
        };
        menu = menu.item(label, action);
    }
    menu
}

fn append_path_section(
    mut menu: ContextMenuBuilder,
    include_relative_path: bool,
) -> ContextMenuBuilder {
    menu = menu.item("Copy Path", "copy-path");
    if include_relative_path {
        menu = menu.item("Copy Relative Path", "copy-relative-path");
    }
    menu
}

fn append_destructive_section(menu: ContextMenuBuilder) -> ContextMenuBuilder {
    menu.item("Rename...", "rename").item("Delete...", "delete")
}

fn append_ignore_section(
    mut menu: ContextMenuBuilder,
    path: &str,
    kind: IgnoreTargetKind,
) -> ContextMenuBuilder {
    let options = gitignore::options_for_path(path, kind);
    if let Some(option) = options.direct {
        menu = menu.target_item(&option.label, "ignore-pattern", &option.pattern);
    }
    if !options.folders.is_empty() {
        let mut folders = context_menu::builder("repo_file");
        for option in options.folders {
            folders = folders.target_item(&option.label, "ignore-pattern", &option.pattern);
        }
        menu = menu.submenu("Ignore Folder (Add to .gitignore)", &folders.build());
    }
    if let Some(option) = options.extension {
        menu = menu.target_item(&option.label, "ignore-pattern", &option.pattern);
    }
    menu
}
