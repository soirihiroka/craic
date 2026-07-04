use super::{GRADLE_ICON_NAME, RunCommand, RunItem};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(super) fn gradle_project_path(repo_path: &Path) -> Option<PathBuf> {
    const GRADLE_FILES: [&str; 4] = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ];

    for name in GRADLE_FILES {
        let path = repo_path.join(name);
        if path.is_file() {
            return Some(path);
        }
    }

    if gradlew_path(repo_path).is_some() {
        return Some(repo_path.join("gradlew"));
    }

    if let Ok(entries) = fs::read_dir(repo_path) {
        for entry in entries.filter_map(Result::ok) {
            let module_root = entry.path();
            if !module_root.is_dir() {
                continue;
            }

            for name in GRADLE_FILES {
                let path = module_root.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    None
}

fn gradlew_path(repo_path: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        let path = repo_path.join("gradlew.bat");
        if path.is_file() {
            return Some(path);
        }
    }

    let path = repo_path.join("gradlew");
    if path.is_file() {
        return Some(path);
    }
    None
}

fn gradle_command(repo_path: &Path) -> String {
    if gradlew_path(repo_path).is_some() {
        if cfg!(windows) {
            "gradlew.bat".to_string()
        } else {
            "./gradlew".to_string()
        }
    } else {
        "gradle".to_string()
    }
}

pub(super) fn android_manifest_path(repo_path: &Path) -> Option<PathBuf> {
    android_manifest_candidates(repo_path).into_iter().next()
}

fn android_manifest_candidates(repo_path: &Path) -> Vec<PathBuf> {
    const MANIFEST_PATHS: [&str; 2] = [
        "app/src/main/AndroidManifest.xml",
        "src/main/AndroidManifest.xml",
    ];
    let mut manifests = Vec::new();

    for path in MANIFEST_PATHS {
        let manifest_path = repo_path.join(path);
        if manifest_path.is_file() {
            manifests.push(manifest_path);
        }
    }

    if let Ok(entries) = fs::read_dir(repo_path) {
        for entry in entries.filter_map(Result::ok) {
            let module_path = entry.path();
            if !module_path.is_dir() {
                continue;
            }

            let manifest_path = module_path.join("src/main/AndroidManifest.xml");
            if manifest_path.is_file() {
                manifests.push(manifest_path);
            }
        }
    }

    for entry in WalkDir::new(repo_path)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "AndroidManifest.xml")
    {
        manifests.push(entry.path().to_path_buf());
    }

    manifests
}

fn android_manifest_package_name_from_contents(contents: &str) -> Option<String> {
    let manifest_open_tag = Regex::new(r#"(?s)<manifest\s+[^>]*>"#).ok()?;
    let manifest_open = manifest_open_tag.find(contents)?.as_str();

    let package_re = Regex::new(r#"package\s*=\s*["']([^"']+)["']"#).ok()?;
    package_re
        .captures(manifest_open)
        .and_then(|captures| captures.get(1))
        .map(|package_name| package_name.as_str().to_string())
}

fn android_manifest_attribute(tag: &str, attribute: &str) -> Option<String> {
    let attribute_re = Regex::new(&format!(
        r#"{}\s*=\s*["']([^"']+)["']"#,
        regex::escape(attribute)
    ))
    .ok()?;
    attribute_re
        .captures(tag)
        .and_then(|captures| captures.get(1))
        .map(|name| name.as_str().trim().to_string())
}

fn android_manifest_launch_component(repo_path: &Path) -> Option<String> {
    android_manifest_candidates(repo_path)
        .into_iter()
        .find_map(|manifest_path| {
            let manifest = fs::read_to_string(manifest_path).ok()?;
            let package_name = android_manifest_package_name_from_contents(&manifest)?;
            android_manifest_launch_component_from_contents(&manifest, &package_name)
        })
}

fn android_manifest_launch_component_from_contents(
    manifest: &str,
    package_name: &str,
) -> Option<String> {
    let intent_filter_re = Regex::new(r#"(?s)<intent-filter\b[^>]*>.*?</intent-filter>"#).ok()?;
    let launcher_action_re =
        Regex::new(r#"android:name\s*=\s*["']android\.intent\.action\.MAIN["']"#).ok()?;
    let launcher_category_re =
        Regex::new(r#"android:name\s*=\s*["']android\.intent\.category\.LAUNCHER["']"#).ok()?;

    let activity_re =
        Regex::new(r#"(?s)<activity\b[^>]*>.*?</activity>|<activity\b[^>]*/>"#).ok()?;
    for activity in activity_re.find_iter(manifest) {
        let tag = activity.as_str();
        let has_launcher_intent_filter = if tag.contains("<intent-filter") {
            intent_filter_re.find_iter(tag).any(|intent_filter| {
                launcher_action_re.is_match(intent_filter.as_str())
                    && launcher_category_re.is_match(intent_filter.as_str())
            })
        } else {
            false
        };

        if has_launcher_intent_filter {
            let name = android_manifest_attribute(tag, "android:name")?;
            return Some(normalize_android_class_name(package_name, &name));
        }
    }

    let alias_re =
        Regex::new(r#"(?s)<activity-alias\b[^>]*>.*?</activity-alias>|<activity-alias\b[^>]*/>"#)
            .ok()?;
    for alias in alias_re.find_iter(manifest) {
        let tag = alias.as_str();
        let has_launcher_intent_filter = if tag.contains("<intent-filter") {
            intent_filter_re.find_iter(tag).any(|intent_filter| {
                launcher_action_re.is_match(intent_filter.as_str())
                    && launcher_category_re.is_match(intent_filter.as_str())
            })
        } else {
            false
        };

        if has_launcher_intent_filter {
            let name = android_manifest_attribute(tag, "android:targetActivity")
                .or_else(|| android_manifest_attribute(tag, "android:name"));
            if let Some(name) = name {
                return Some(normalize_android_class_name(package_name, &name));
            }
        }
    }

    None
}

fn normalize_android_class_name(package_name: &str, class_name: &str) -> String {
    if class_name.starts_with('.') {
        format!("{package_name}/{class_name}")
    } else if class_name.contains('.') {
        format!("{package_name}/{class_name}")
    } else {
        format!("{package_name}/{package_name}.{class_name}")
    }
}

pub(super) fn discover_gradle_targets(repo_path: &Path) -> Vec<RunItem> {
    if gradle_project_path(repo_path).is_none() {
        return Vec::new();
    }

    let gradle_program = gradle_command(repo_path);
    let is_android = android_manifest_path(repo_path).is_some();
    let mut targets = vec![RunItem {
        id: "gradle:build".to_string(),
        label: "Build (Gradle)".to_string(),
        icon_name: GRADLE_ICON_NAME.to_string(),
        command: RunCommand::ShellCommand {
            command: format!("{gradle_program} build"),
        },
    }];

    if is_android {
        targets.push(RunItem {
            id: "gradle:assemble-debug".to_string(),
            label: "Build Debug APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} assembleDebug"),
            },
        });
        targets.push(RunItem {
            id: "gradle:assemble-release".to_string(),
            label: "Build Release APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} assembleRelease"),
            },
        });
        targets.push(RunItem {
            id: "gradle:install-debug".to_string(),
            label: "Install Debug APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} installDebug"),
            },
        });
        targets.push(RunItem {
            id: "gradle:install-release".to_string(),
            label: "Install Release APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} installRelease"),
            },
        });
        targets.push(RunItem {
            id: "gradle:debug-build-install-launch".to_string(),
            label: "Run (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: adb_debug_run_command(repo_path, &gradle_program),
            },
        });
    }

    targets
}

fn adb_debug_run_command(repo_path: &Path, gradle_program: &str) -> String {
    let Some(component) = android_manifest_launch_component(repo_path) else {
        return include_str!("android_debug_run_missing_launcher.sh")
            .replace("__GRADLE_PROGRAM__", gradle_program);
    };
    let component = shell_quote(&component);

    include_str!("android_debug_run.sh")
        .replace("__GRADLE_PROGRAM__", gradle_program)
        .replace("__COMPONENT__", &component)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
