pub use craic_system::system;
pub use craic_vcs::git;

pub mod ui {
    pub use craic_ui_core::ui::components;

    pub mod content {
        pub use craic_ui_editor::code_editor;
    }

    pub mod pages {
        pub use craic_ui_core::ui::pages::*;

        mod containers;

        pub use containers::ContainersPage;
    }
}

pub use ui::pages::ContainersPage;
