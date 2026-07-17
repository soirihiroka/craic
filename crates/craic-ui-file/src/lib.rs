pub use craic_config as config;
pub use craic_language::{markdown_lint, spellcheck};
pub use craic_project::workspace_config;
pub use craic_system::system;
pub use craic_vcs::{git, gitignore};

pub mod ui {
    pub use craic_ui_core::ui::{canvas_scrollbar, components, file_status, file_type, widgets};

    pub mod content {
        pub use craic_ui_editor::code_editor;
        pub use craic_ui_preview::{binary_preview, folder_view};
    }

    pub mod sidebar {
        pub mod file_browser;
    }

    pub mod pages {
        pub use craic_ui_core::ui::pages::*;

        mod file;

        pub use file::FilePage;
    }
}

pub use ui::pages::FilePage;
