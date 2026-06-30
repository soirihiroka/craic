use adw::prelude::*;
use std::os::unix::fs::MetadataExt;

pub(in crate::ui) struct FolderView {
    pub(in crate::ui) root: gtk::ScrolledWindow,
    title_label: gtk::Label,
    repo_path_row: adw::ActionRow,
    abs_path_row: adw::ActionRow,
    files_row: adw::ActionRow,
    folders_row: adw::ActionRow,
    items_row: adw::ActionRow,
    owner_row: adw::ActionRow,
    group_row: adw::ActionRow,
    permissions_row: adw::ActionRow,
    size_row: adw::ActionRow,
    modified_row: adw::ActionRow,
}

impl FolderView {
    pub(in crate::ui) fn new() -> Self {
        // Header Icon & Title
        let header_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .margin_bottom(12)
            .build();

        let icon_image = gtk::Image::builder()
            .icon_name("folder-symbolic")
            .pixel_size(24)
            .halign(gtk::Align::Start)
            .build();

        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .css_classes(["heading", "bold"])
            .build();

        header_box.append(&icon_image);
        header_box.append(&title_label);

        // Location Group
        let repo_path_row = adw::ActionRow::builder()
            .title("Workspace Path")
            .subtitle("Workspace root")
            .build();
        let abs_path_row = adw::ActionRow::builder()
            .title("Location")
            .subtitle("Workspace root")
            .build();

        let location_group = adw::PreferencesGroup::builder().title("Location").build();
        location_group.add(&repo_path_row);
        location_group.add(&abs_path_row);

        // Contents Group
        let files_row = adw::ActionRow::builder()
            .title("Files")
            .subtitle("0 files")
            .build();
        let folders_row = adw::ActionRow::builder()
            .title("Folders")
            .subtitle("0 folders")
            .build();
        let items_row = adw::ActionRow::builder()
            .title("Total Items")
            .subtitle("0 items")
            .build();

        let contents_group = adw::PreferencesGroup::builder().title("Contents").build();
        contents_group.add(&files_row);
        contents_group.add(&folders_row);
        contents_group.add(&items_row);

        // System Metadata Group
        let owner_row = adw::ActionRow::builder()
            .title("Owner")
            .subtitle("Unknown")
            .build();
        let group_row = adw::ActionRow::builder()
            .title("Group")
            .subtitle("Unknown")
            .build();
        let permissions_row = adw::ActionRow::builder()
            .title("Permissions")
            .subtitle("Unknown")
            .build();
        let size_row = adw::ActionRow::builder()
            .title("Size")
            .subtitle("Unknown")
            .build();
        let modified_row = adw::ActionRow::builder()
            .title("Last Modified")
            .subtitle("Unknown")
            .build();

        let metadata_group = adw::PreferencesGroup::builder()
            .title("System Metadata")
            .build();
        metadata_group.add(&owner_row);
        metadata_group.add(&group_row);
        metadata_group.add(&permissions_row);
        metadata_group.add(&size_row);
        metadata_group.add(&modified_row);

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(24)
            .margin_end(24)
            .hexpand(true)
            .build();
        panel.append(&header_box);
        panel.append(&location_group);
        panel.append(&contents_group);
        panel.append(&metadata_group);

        let clamp = adw::Clamp::builder()
            .orientation(gtk::Orientation::Horizontal)
            .maximum_size(620)
            .tightening_threshold(520)
            .hexpand(true)
            .child(&panel)
            .build();

        let root = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(false)
            .child(&clamp)
            .build();

        Self {
            root,
            title_label,
            repo_path_row,
            abs_path_row,
            files_row,
            folders_row,
            items_row,
            owner_row,
            group_row,
            permissions_row,
            size_row,
            modified_row,
        }
    }

    pub(in crate::ui) fn set_info(
        &self,
        workspace_root: &str,
        folder_path: &str,
        file_count: usize,
        folder_count: usize,
    ) {
        let abs_path = if folder_path.is_empty() {
            workspace_root.to_string()
        } else {
            format!(
                "{}/{}",
                workspace_root.trim_end_matches('/'),
                folder_path.trim_start_matches('/')
            )
        };
        let folder_name = if folder_path.is_empty() {
            "Workspace root".to_string()
        } else {
            folder_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(folder_path)
                .to_string()
        };

        self.title_label.set_text(&folder_name);

        let repo_path_str = if folder_path.is_empty() {
            "Workspace root".to_string()
        } else {
            folder_path.to_string()
        };
        self.repo_path_row.set_subtitle(&repo_path_str);
        self.abs_path_row.set_subtitle(&abs_path);

        let entry_count = file_count + folder_count;
        self.files_row.set_subtitle(&format!("{file_count} files"));
        self.folders_row
            .set_subtitle(&format!("{folder_count} folders"));
        self.items_row.set_subtitle(&format!("{entry_count} items"));

        if let Ok(meta) = std::fs::metadata(&abs_path) {
            let uid = meta.uid();
            let gid = meta.gid();
            let mode = meta.mode();
            let size = meta.size();
            let mtime = meta.mtime();

            self.owner_row.set_subtitle(&get_username(uid));
            self.group_row.set_subtitle(&get_groupname(gid));
            self.permissions_row.set_subtitle(&format_permissions(mode));
            self.size_row.set_subtitle(&format_size(size));
            self.modified_row.set_subtitle(&format_system_time(mtime));
        } else {
            self.owner_row.set_subtitle("Unknown");
            self.group_row.set_subtitle("Unknown");
            self.permissions_row.set_subtitle("Unknown");
            self.size_row.set_subtitle("Unknown");
            self.modified_row.set_subtitle("Unknown");
        }
    }
}

fn get_username(uid: u32) -> String {
    unsafe {
        let pwd = libc::getpwuid(uid);
        if !pwd.is_null() {
            let name_ptr = (*pwd).pw_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    uid.to_string()
}

fn get_groupname(gid: u32) -> String {
    unsafe {
        let grp = libc::getgrgid(gid);
        if !grp.is_null() {
            let name_ptr = (*grp).gr_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    gid.to_string()
}

fn format_permissions(mode: u32) -> String {
    let octal = format!("{:04o}", mode & 0o777);
    let mut rwx = String::with_capacity(10);
    rwx.push('d');
    let rwx_chars = ['r', 'w', 'x'];
    for i in (0..9).rev() {
        if (mode & (1 << i)) != 0 {
            rwx.push(rwx_chars[(8 - i) % 3]);
        } else {
            rwx.push('-');
        }
    }
    format!("{} ({})", rwx, octal)
}

fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if size >= GB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.2} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

fn format_system_time(mtime_secs: i64) -> String {
    unsafe {
        let mut time_str = [0; 64];
        let tm = libc::localtime(&mtime_secs);
        if !tm.is_null() {
            let format = std::ffi::CString::new("%Y-%m-%d %H:%M:%S").unwrap();
            let len = libc::strftime(time_str.as_mut_ptr(), time_str.len(), format.as_ptr(), tm);
            if len > 0 {
                if let Ok(s) = std::ffi::CStr::from_ptr(time_str.as_ptr()).to_str() {
                    return s.to_string();
                }
            }
        }
    }
    "Unknown".to_string()
}
