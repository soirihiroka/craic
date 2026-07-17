pub use craic_agent::{agent_provider, ai_commit};
pub use craic_config as config;
pub use craic_system::system;
pub use craic_vcs::{git, github, gitignore};

mod suggestions;

pub mod ui {
    pub use craic_ui_core::ui::{
        canvas_scroll, components, file_row, file_type, left_clamp, widgets,
    };

    pub mod content {
        pub use crate::suggestions::{SuggestionsActions, SuggestionsPanel, centered_page, page};
        pub use craic_ui_editor::diff_view;
        pub use craic_ui_preview::binary_preview;
    }

    pub mod file_manager;

    pub mod sidebar {
        pub mod changes;
        pub mod changes_panel;
        pub mod commit_panel;
        pub mod history;
    }

    pub mod pages {
        pub use craic_ui_core::ui::pages::*;

        mod changes;
        mod history;
        mod preview_reconcile;

        pub use changes::ChangesPage;
        pub use history::HistoryPage;
    }
}

pub use ui::pages::{ChangesPage, HistoryPage};
