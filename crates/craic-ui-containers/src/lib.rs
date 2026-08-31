pub use craic_system::system;
#[path = "ui/pages/containers/docker.rs"]
pub mod docker;

#[cfg(target_os = "linux")]
pub use craic_vcs::git;

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub use ui::pages::ContainersPage;
