use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MIN_FONT_SIZE: f64 = 8.0;
pub const MAX_FONT_SIZE: f64 = 32.0;
pub const DEFAULT_SHELL_FONT_SIZE: f64 = 13.0;
pub const DEFAULT_EDITOR_FONT_SIZE: f64 = 14.0;
pub const DEFAULT_DIFF_FONT_SIZE: f64 = 13.0;
pub const DEFAULT_AGENT_PROVIDER_ID: &str = "opencode";

const WORKSPACES_KEY: &str = "workspaces";
const WORKSPACE_ROOTS_KEY: &str = "workspace_roots";
const HOSTS_KEY: &str = "hosts";
const HOST_COLORS_PREFIX: &str = "host_colors.";
const COMMIT_MESSAGE_PROVIDER_KEY: &str = "commit_message_provider";
const COMMIT_MESSAGE_MODEL_KEY: &str = "commit_message_model";
const SMART_FEATURE_PREFIX: &str = "smart_feature";
const OLLAMA_BASE_URL_KEY: &str = "ollama.base_url";
const SHELL_FONT_SIZE_KEY: &str = "font_size.shell";
const EDITOR_FONT_SIZE_KEY: &str = "font_size.editor";
const DIFF_FONT_SIZE_KEY: &str = "font_size.diff";
const COLOR_KEY: &str = "color";

type ConfigMap = HashMap<String, toml::Value>;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub workspace_roots: Vec<ConfiguredWorkspace>,
    pub workspaces: Vec<ConfiguredWorkspace>,
    pub host_colors: HashMap<String, WorkspaceColor>,
    pub commit_message_provider: String,
    pub commit_message_model: Option<String>,
    pub smart_features: HashMap<String, SmartFeatureConfig>,
    pub ollama_base_url: Option<String>,
    pub font_sizes: FontSizes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredWorkspace {
    pub path: String,
    pub provider: WorkspaceProvider,
    pub display_name: Option<String>,
    pub color: Option<WorkspaceColor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceProvider {
    Local,
    Ssh { host: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceColor {
    pub background: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartFeatureConfig {
    pub provider: String,
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct FontSizes {
    pub shell: f64,
    pub editor: f64,
    pub diff: f64,
}

impl Default for FontSizes {
    fn default() -> Self {
        Self {
            shell: DEFAULT_SHELL_FONT_SIZE,
            editor: DEFAULT_EDITOR_FONT_SIZE,
            diff: DEFAULT_DIFF_FONT_SIZE,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StateFile {
    last_repo: Option<String>,
    last_workspace_path: Option<String>,
    last_workspace_provider: Option<String>,
}

pub fn load() -> AppConfig {
    let config = load_merged_config();

    let mut workspace_roots = config_workspace_array(&config, WORKSPACE_ROOTS_KEY);

    if workspace_roots.is_empty() {
        if let Some(default_root) = default_repo_root() {
            workspace_roots.push(ConfiguredWorkspace::local(default_root.to_string_lossy()));
        }
    }
    let workspaces = config_workspace_array(&config, WORKSPACES_KEY);
    let host_colors = config_host_colors(&config);
    let commit_message_provider = normalized_config_string(
        config_string(&config, COMMIT_MESSAGE_PROVIDER_KEY)
            .unwrap_or_else(|| DEFAULT_AGENT_PROVIDER_ID.to_string()),
    )
    .unwrap_or_else(|| DEFAULT_AGENT_PROVIDER_ID.to_string());
    let commit_message_model =
        normalized_commit_message_model(config_string(&config, COMMIT_MESSAGE_MODEL_KEY));
    let smart_features = smart_feature_configs(&config);
    let ollama_base_url =
        config_string(&config, OLLAMA_BASE_URL_KEY).and_then(normalized_config_string);
    let font_sizes = FontSizes {
        shell: normalize_font_size(
            config_f64(&config, SHELL_FONT_SIZE_KEY).unwrap_or(DEFAULT_SHELL_FONT_SIZE),
            DEFAULT_SHELL_FONT_SIZE,
        ),
        editor: normalize_font_size(
            config_f64(&config, EDITOR_FONT_SIZE_KEY).unwrap_or(DEFAULT_EDITOR_FONT_SIZE),
            DEFAULT_EDITOR_FONT_SIZE,
        ),
        diff: normalize_font_size(
            config_f64(&config, DIFF_FONT_SIZE_KEY).unwrap_or(DEFAULT_DIFF_FONT_SIZE),
            DEFAULT_DIFF_FONT_SIZE,
        ),
    };

    AppConfig {
        workspace_roots,
        workspaces,
        host_colors,
        commit_message_provider,
        commit_message_model,
        smart_features,
        ollama_base_url,
        font_sizes,
    }
}

impl ConfiguredWorkspace {
    pub fn local(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            provider: WorkspaceProvider::Local,
            display_name: None,
            color: None,
        }
    }

    pub fn provider_id(&self) -> String {
        self.provider.id()
    }

    pub fn selection_id(&self) -> String {
        format!("{}|{}", self.provider_id(), self.path)
    }

    pub fn label(&self) -> String {
        self.display_name.clone().unwrap_or_else(|| {
            let trimmed = self.path.trim_end_matches('/');
            trimmed
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(trimmed)
                .to_string()
        })
    }
}

impl WorkspaceProvider {
    pub fn parse(value: Option<&str>) -> Self {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self::Local;
        };
        match value {
            "local" | "localhost" => Self::Local,
            _ => {
                if let Some(host) = value.strip_prefix("ssh:").filter(|host| !host.is_empty()) {
                    Self::Ssh {
                        host: host.to_string(),
                    }
                } else {
                    log::warn!("unknown workspace provider '{value}', defaulting to local");
                    Self::Local
                }
            }
        }
    }

    pub fn id(&self) -> String {
        match self {
            Self::Local => "local".to_string(),
            Self::Ssh { host } => format!("ssh:{host}"),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

pub fn smart_feature_config(shell_provider_id: &str) -> SmartFeatureConfig {
    load()
        .smart_features
        .get(shell_provider_id)
        .cloned()
        .unwrap_or_else(|| SmartFeatureConfig {
            provider: shell_provider_id.to_string(),
            model: None,
        })
}

pub fn workspace_color_for(provider_id: &str, workspace_root: &str) -> Option<WorkspaceColor> {
    let config = load();
    let workspace_root = normalize_target_path(workspace_root);
    let mut resolved: Option<(WorkspaceColor, String)> = None;

    if let Some(host) = provider_id.strip_prefix("ssh:")
        && let Some(color) = config.host_colors.get(host)
    {
        resolved = Some((color.clone(), format!("host:{host}")));
    }

    for root in &config.workspace_roots {
        if root.provider_id() == provider_id
            && let Some(color) = root.color.as_ref()
            && let Some(root_path) = config_path_for_match(&root.path, &root.provider)
            && target_path_contains(&root_path, &workspace_root)
        {
            resolved = Some((color.clone(), format!("workspace_root:{}", root.path)));
        }
    }

    for workspace in &config.workspaces {
        if workspace.provider_id() == provider_id
            && let Some(color) = workspace.color.as_ref()
            && let Some(path) = config_path_for_match(&workspace.path, &workspace.provider)
            && target_path_matches(&path, &workspace_root)
        {
            resolved = Some((color.clone(), format!("workspace:{}", workspace.path)));
        }
    }

    match resolved {
        Some((color, source)) => {
            log::debug!(
                "workspace color resolved provider={} root={} source={} color={}",
                provider_id,
                workspace_root,
                source,
                color.background
            );
            Some(color)
        }
        None => {
            log::debug!(
                "workspace color not configured provider={} root={}",
                provider_id,
                workspace_root
            );
            None
        }
    }
}

pub fn save_commit_message_provider(provider_id: &str) {
    let mut config = load_user_config();
    config.insert(
        COMMIT_MESSAGE_PROVIDER_KEY.to_string(),
        toml::Value::String(normalized_provider_id(provider_id)),
    );
    config.remove(COMMIT_MESSAGE_MODEL_KEY);
    save_config_file(&config);
}

pub fn save_commit_message_model(provider_id: &str, model: Option<&str>) {
    let mut config = load_user_config();
    config.insert(
        COMMIT_MESSAGE_PROVIDER_KEY.to_string(),
        toml::Value::String(normalized_provider_id(provider_id)),
    );
    match normalized_commit_message_model(model.map(ToString::to_string)) {
        Some(model) => {
            config.insert(
                COMMIT_MESSAGE_MODEL_KEY.to_string(),
                toml::Value::String(model),
            );
        }
        None => {
            config.remove(COMMIT_MESSAGE_MODEL_KEY);
        }
    }
    save_config_file(&config);
}

pub fn save_smart_feature_provider(shell_provider_id: &str, provider_id: &str) {
    let mut config = load_user_config();
    let provider_key = smart_feature_provider_key(shell_provider_id);
    let model_key = smart_feature_model_key(shell_provider_id);
    config.insert(
        provider_key,
        toml::Value::String(normalized_provider_id(provider_id)),
    );
    config.remove(&model_key);
    save_config_file(&config);
}

pub fn save_smart_feature_model(shell_provider_id: &str, provider_id: &str, model: Option<&str>) {
    let mut config = load_user_config();
    config.insert(
        smart_feature_provider_key(shell_provider_id),
        toml::Value::String(normalized_provider_id(provider_id)),
    );
    match normalized_commit_message_model(model.map(ToString::to_string)) {
        Some(model) => {
            config.insert(
                smart_feature_model_key(shell_provider_id),
                toml::Value::String(model),
            );
        }
        None => {
            config.remove(&smart_feature_model_key(shell_provider_id));
        }
    }
    save_config_file(&config);
}

fn normalized_provider_id(provider_id: &str) -> String {
    normalized_config_string(provider_id.to_string())
        .unwrap_or_else(|| DEFAULT_AGENT_PROVIDER_ID.to_string())
}

fn normalized_commit_message_model(model: Option<String>) -> Option<String> {
    model.and_then(normalized_config_string)
}

fn smart_feature_configs(config: &ConfigMap) -> HashMap<String, SmartFeatureConfig> {
    smart_feature_shell_providers()
        .iter()
        .map(|shell_provider| {
            let provider = config_string(config, &smart_feature_provider_key(shell_provider))
                .and_then(normalized_config_string)
                .unwrap_or_else(|| (*shell_provider).to_string());
            let model = normalized_commit_message_model(config_string(
                config,
                &smart_feature_model_key(shell_provider),
            ));
            (
                (*shell_provider).to_string(),
                SmartFeatureConfig { provider, model },
            )
        })
        .collect()
}

pub fn smart_feature_shell_providers() -> &'static [&'static str] {
    &["codex", "agy", "opencode"]
}

fn smart_feature_provider_key(shell_provider_id: &str) -> String {
    format!("{SMART_FEATURE_PREFIX}.{shell_provider_id}.provider")
}

fn smart_feature_model_key(shell_provider_id: &str) -> String {
    format!("{SMART_FEATURE_PREFIX}.{shell_provider_id}.model")
}

fn normalized_config_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn save_font_sizes(font_sizes: FontSizes) {
    let mut config = load_user_config();
    config.insert(
        SHELL_FONT_SIZE_KEY.to_string(),
        toml::Value::Float(normalize_font_size(
            font_sizes.shell,
            DEFAULT_SHELL_FONT_SIZE,
        )),
    );
    config.insert(
        EDITOR_FONT_SIZE_KEY.to_string(),
        toml::Value::Float(normalize_font_size(
            font_sizes.editor,
            DEFAULT_EDITOR_FONT_SIZE,
        )),
    );
    config.insert(
        DIFF_FONT_SIZE_KEY.to_string(),
        toml::Value::Float(normalize_font_size(font_sizes.diff, DEFAULT_DIFF_FONT_SIZE)),
    );
    save_config_file(&config);
}

pub fn save_shell_font_size(font_size: f64) {
    let mut font_sizes = load().font_sizes;
    font_sizes.shell = normalize_font_size(font_size, DEFAULT_SHELL_FONT_SIZE);
    save_font_sizes(font_sizes);
}

pub fn save_editor_font_size(font_size: f64) {
    let mut font_sizes = load().font_sizes;
    font_sizes.editor = normalize_font_size(font_size, DEFAULT_EDITOR_FONT_SIZE);
    save_font_sizes(font_sizes);
}

pub fn save_diff_font_size(font_size: f64) {
    let mut font_sizes = load().font_sizes;
    font_sizes.diff = normalize_font_size(font_size, DEFAULT_DIFF_FONT_SIZE);
    save_font_sizes(font_sizes);
}

pub fn normalize_font_size(value: f64, default: f64) -> f64 {
    if value.is_finite() {
        value.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        default
    }
}

pub fn last_workspace() -> Option<ConfiguredWorkspace> {
    let state = state_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| toml::from_str::<StateFile>(&contents).ok())?;

    let path = state
        .last_workspace_path
        .or(state.last_repo)
        .and_then(|path| normalized_config_string(path))?;
    Some(ConfiguredWorkspace {
        path,
        provider: WorkspaceProvider::parse(state.last_workspace_provider.as_deref()),
        display_name: None,
        color: None,
    })
}

pub fn save_last_workspace(workspace: &ConfiguredWorkspace) {
    let Some(state_path) = state_path() else {
        return;
    };

    if let Some(parent) = state_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let state = StateFile {
        last_repo: None,
        last_workspace_path: Some(workspace.path.clone()),
        last_workspace_provider: Some(workspace.provider_id()),
    };
    if let Ok(contents) = toml::to_string_pretty(&state) {
        let _ = std::fs::write(state_path, contents);
    }
}

fn load_merged_config() -> ConfigMap {
    let mut config = default_config();
    config.extend(load_user_config());
    config
}

fn default_config() -> ConfigMap {
    HashMap::from([
        (
            WORKSPACE_ROOTS_KEY.to_string(),
            toml::Value::Array(vec![toml::Value::String("~/Repos".to_string())]),
        ),
        (
            COMMIT_MESSAGE_PROVIDER_KEY.to_string(),
            toml::Value::String(DEFAULT_AGENT_PROVIDER_ID.to_string()),
        ),
        (
            SHELL_FONT_SIZE_KEY.to_string(),
            toml::Value::Float(DEFAULT_SHELL_FONT_SIZE),
        ),
        (
            EDITOR_FONT_SIZE_KEY.to_string(),
            toml::Value::Float(DEFAULT_EDITOR_FONT_SIZE),
        ),
        (
            DIFF_FONT_SIZE_KEY.to_string(),
            toml::Value::Float(DEFAULT_DIFF_FONT_SIZE),
        ),
    ])
}

fn load_user_config() -> ConfigMap {
    config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
        .and_then(|value| value.as_table().cloned())
        .map(flatten_table)
        .unwrap_or_default()
}

fn flatten_table(table: toml::Table) -> ConfigMap {
    let mut config = ConfigMap::new();
    for (key, value) in table {
        flatten_value(key, value, &mut config);
    }
    config
}

fn flatten_value(key: String, value: toml::Value, config: &mut ConfigMap) {
    match value {
        toml::Value::Table(table) => {
            for (child_key, child_value) in table {
                flatten_value(format!("{key}.{child_key}"), child_value, config);
            }
        }
        value => {
            config.insert(key, value);
        }
    }
}

fn save_config_file(config: &ConfigMap) {
    let Some(path) = config_path() else {
        return;
    };

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let contents = unflatten_config(config);
    if let Ok(contents) = toml::to_string_pretty(&contents) {
        let _ = std::fs::write(path, contents);
    }
}

fn unflatten_config(config: &ConfigMap) -> toml::Table {
    let mut root = toml::Table::new();
    for (key, value) in config {
        insert_flat_value(&mut root, key, value.clone());
    }
    root
}

fn insert_flat_value(root: &mut toml::Table, key: &str, value: toml::Value) {
    let Some((parents, last)) = key.rsplit_once('.') else {
        root.insert(key.to_string(), value);
        return;
    };

    let mut table = root;
    for part in parents.split('.') {
        let entry = table
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        table = entry.as_table_mut().expect("entry was set to a table");
    }
    table.insert(last.to_string(), value);
}

fn config_string(config: &ConfigMap, key: &str) -> Option<String> {
    config.get(key)?.as_str().map(ToString::to_string)
}

fn config_workspace_array(config: &ConfigMap, key: &str) -> Vec<ConfiguredWorkspace> {
    config
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| configured_workspace_from_value(value, key))
                .collect()
        })
        .unwrap_or_default()
}

fn configured_workspace_from_value(
    value: &toml::Value,
    source: &str,
) -> Option<ConfiguredWorkspace> {
    match value {
        toml::Value::String(path) => {
            normalized_config_string(path.clone()).map(|path| ConfiguredWorkspace {
                path,
                provider: WorkspaceProvider::Local,
                display_name: None,
                color: None,
            })
        }
        toml::Value::Table(table) => {
            let path = table
                .get("path")
                .and_then(toml::Value::as_str)
                .and_then(|path| normalized_config_string(path.to_string()))?;
            let provider =
                WorkspaceProvider::parse(table.get("provider").and_then(toml::Value::as_str));
            let display_name = table
                .get("name")
                .and_then(toml::Value::as_str)
                .and_then(|name| normalized_config_string(name.to_string()));
            let color = config_table_color(table, source);
            Some(ConfiguredWorkspace {
                path,
                provider,
                display_name,
                color,
            })
        }
        _ => None,
    }
}

fn config_host_colors(config: &ConfigMap) -> HashMap<String, WorkspaceColor> {
    let mut colors = HashMap::new();

    if let Some(hosts) = config.get(HOSTS_KEY).and_then(toml::Value::as_array) {
        for host in hosts {
            let toml::Value::Table(table) = host else {
                continue;
            };
            let Some(host) = table
                .get("host")
                .and_then(toml::Value::as_str)
                .and_then(|host| normalized_config_string(host.to_string()))
            else {
                continue;
            };
            if let Some(color) = config_table_color(table, &format!("hosts.{host}")) {
                colors.insert(host, color);
            }
        }
    }

    for (key, value) in config {
        let Some(host) = key.strip_prefix(HOST_COLORS_PREFIX) else {
            continue;
        };
        let host = host.strip_suffix(".color").unwrap_or(host);
        let Some(host) = normalized_config_string(host.to_string()) else {
            continue;
        };
        let Some(value) = value.as_str() else {
            log::warn!("workspace host color ignored key={key} reason=not-a-string");
            continue;
        };
        if let Some(color) = normalized_workspace_color(value, key) {
            colors.insert(host, color);
        }
    }

    colors
}

fn config_table_color(table: &toml::Table, source: &str) -> Option<WorkspaceColor> {
    table
        .get(COLOR_KEY)
        .and_then(toml::Value::as_str)
        .and_then(|color| normalized_workspace_color(color, &format!("{source}.{COLOR_KEY}")))
}

fn normalized_workspace_color(value: &str, source: &str) -> Option<WorkspaceColor> {
    let value = value.trim();
    if let Some(color) = adwaita_named_workspace_color(value) {
        return Some(WorkspaceColor {
            background: color.to_string(),
        });
    }

    let hex = value.strip_prefix('#').unwrap_or(value);
    let valid_len = matches!(hex.len(), 3 | 4 | 6 | 8);
    if valid_len && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(WorkspaceColor {
            background: format!("#{hex}"),
        });
    }

    log::warn!(
        "workspace color ignored key={} value={} reason=expected-hex-or-adwaita-color",
        source,
        value
    );
    None
}

fn adwaita_named_workspace_color(value: &str) -> Option<&'static str> {
    let name = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    match name.as_str() {
        "blue" | "adw-blue" | "adwaita-blue" | "blue-3" | "adw-blue-3" | "adwaita-blue-3" => {
            Some("#3584e4")
        }
        "blue-1" | "adw-blue-1" | "adwaita-blue-1" => Some("#99c1f1"),
        "blue-2" | "adw-blue-2" | "adwaita-blue-2" => Some("#62a0ea"),
        "blue-4" | "adw-blue-4" | "adwaita-blue-4" => Some("#1c71d8"),
        "blue-5" | "adw-blue-5" | "adwaita-blue-5" => Some("#1a5fb4"),
        "green" | "adw-green" | "adwaita-green" | "green-3" | "adw-green-3" | "adwaita-green-3" => {
            Some("#33d17a")
        }
        "green-1" | "adw-green-1" | "adwaita-green-1" => Some("#8ff0a4"),
        "green-2" | "adw-green-2" | "adwaita-green-2" => Some("#57e389"),
        "green-4" | "adw-green-4" | "adwaita-green-4" => Some("#2ec27e"),
        "green-5" | "adw-green-5" | "adwaita-green-5" => Some("#26a269"),
        "yellow" | "adw-yellow" | "adwaita-yellow" | "yellow-3" | "adw-yellow-3"
        | "adwaita-yellow-3" => Some("#f6d32d"),
        "yellow-1" | "adw-yellow-1" | "adwaita-yellow-1" => Some("#f9f06b"),
        "yellow-2" | "adw-yellow-2" | "adwaita-yellow-2" => Some("#f8e45c"),
        "yellow-4" | "adw-yellow-4" | "adwaita-yellow-4" => Some("#f5c211"),
        "yellow-5" | "adw-yellow-5" | "adwaita-yellow-5" => Some("#e5a50a"),
        "orange" | "adw-orange" | "adwaita-orange" | "orange-3" | "adw-orange-3"
        | "adwaita-orange-3" => Some("#ff7800"),
        "orange-1" | "adw-orange-1" | "adwaita-orange-1" => Some("#ffbe6f"),
        "orange-2" | "adw-orange-2" | "adwaita-orange-2" => Some("#ffa348"),
        "orange-4" | "adw-orange-4" | "adwaita-orange-4" => Some("#e66100"),
        "orange-5" | "adw-orange-5" | "adwaita-orange-5" => Some("#c64600"),
        "red" | "adw-red" | "adwaita-red" | "red-3" | "adw-red-3" | "adwaita-red-3" => {
            Some("#e01b24")
        }
        "red-1" | "adw-red-1" | "adwaita-red-1" => Some("#f66151"),
        "red-2" | "adw-red-2" | "adwaita-red-2" => Some("#ed333b"),
        "red-4" | "adw-red-4" | "adwaita-red-4" => Some("#c01c28"),
        "red-5" | "adw-red-5" | "adwaita-red-5" => Some("#a51d2d"),
        "purple" | "adw-purple" | "adwaita-purple" | "purple-3" | "adw-purple-3"
        | "adwaita-purple-3" => Some("#9141ac"),
        "purple-1" | "adw-purple-1" | "adwaita-purple-1" => Some("#dc8add"),
        "purple-2" | "adw-purple-2" | "adwaita-purple-2" => Some("#c061cb"),
        "purple-4" | "adw-purple-4" | "adwaita-purple-4" => Some("#813d9c"),
        "purple-5" | "adw-purple-5" | "adwaita-purple-5" => Some("#613583"),
        "brown" | "adw-brown" | "adwaita-brown" | "brown-3" | "adw-brown-3" | "adwaita-brown-3" => {
            Some("#986a44")
        }
        "brown-1" | "adw-brown-1" | "adwaita-brown-1" => Some("#cdab8f"),
        "brown-2" | "adw-brown-2" | "adwaita-brown-2" => Some("#b5835a"),
        "brown-4" | "adw-brown-4" | "adwaita-brown-4" => Some("#865e3c"),
        "brown-5" | "adw-brown-5" | "adwaita-brown-5" => Some("#63452c"),
        "light" | "adw-light" | "adwaita-light" | "light-3" | "adw-light-3" | "adwaita-light-3" => {
            Some("#deddda")
        }
        "light-1" | "adw-light-1" | "adwaita-light-1" => Some("#ffffff"),
        "light-2" | "adw-light-2" | "adwaita-light-2" => Some("#f6f5f4"),
        "light-4" | "adw-light-4" | "adwaita-light-4" => Some("#c0bfbc"),
        "light-5" | "adw-light-5" | "adwaita-light-5" => Some("#9a9996"),
        "dark" | "adw-dark" | "adwaita-dark" | "dark-3" | "adw-dark-3" | "adwaita-dark-3" => {
            Some("#3d3846")
        }
        "gray" | "grey" | "adw-gray" | "adw-grey" | "adwaita-gray" | "adwaita-grey" | "dark-1"
        | "adw-dark-1" | "adwaita-dark-1" => Some("#77767b"),
        "dark-2" | "adw-dark-2" | "adwaita-dark-2" => Some("#5e5c64"),
        "dark-4" | "adw-dark-4" | "adwaita-dark-4" => Some("#241f31"),
        "dark-5" | "adw-dark-5" | "adwaita-dark-5" => Some("#000000"),
        _ => None,
    }
}

fn config_path_for_match(path: &str, provider: &WorkspaceProvider) -> Option<String> {
    match provider {
        WorkspaceProvider::Local => expand_home(path).map(|path| {
            let path = path.canonicalize().unwrap_or(path);
            normalize_target_path(&path.to_string_lossy())
        }),
        WorkspaceProvider::Ssh { .. } => Some(normalize_target_path(path)),
    }
}

fn normalize_target_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn target_path_matches(config_path: &str, workspace_path: &str) -> bool {
    if config_path == workspace_path {
        return true;
    }

    config_path
        .strip_prefix('~')
        .filter(|suffix| !suffix.is_empty())
        .is_some_and(|suffix| workspace_path.ends_with(suffix))
}

fn target_path_contains(root: &str, path: &str) -> bool {
    if root == "/" {
        return path.starts_with('/');
    }
    if target_path_matches(root, path) {
        return true;
    }
    if let Some(suffix) = root.strip_prefix('~').filter(|suffix| !suffix.is_empty()) {
        let marker = format!("{suffix}/");
        return path.contains(&marker);
    }
    path.strip_prefix(root)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn config_f64(config: &ConfigMap, key: &str) -> Option<f64> {
    let value = config.get(key)?;
    value
        .as_float()
        .or_else(|| value.as_integer().map(|value| value as f64))
}

pub fn config_path() -> Option<PathBuf> {
    Some(craic_dir()?.join("config.toml"))
}

pub fn state_path() -> Option<PathBuf> {
    Some(craic_dir()?.join("state.toml"))
}

pub fn sessions_db_path() -> Option<PathBuf> {
    Some(craic_dir()?.join("sessions.sqlite"))
}

pub fn prompts_dir() -> Option<PathBuf> {
    Some(craic_dir()?.join("prompts"))
}

pub fn prompt_dirs(repo_path: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(prompts_dir) = prompts_dir() {
        dirs.push(prompts_dir);
    }

    if let Some(repo_path) = repo_path {
        let prompts_dir = repo_path.join(".craic").join("prompts");
        if !dirs.iter().any(|dir| dir == &prompts_dir) {
            dirs.push(prompts_dir);
        }
    }

    dirs
}

pub fn expand_config_path_for_ui(path: &str) -> Option<PathBuf> {
    expand_home(path)
}

pub fn craic_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".craic"))
}

fn default_repo_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Repos"))
}

fn expand_home(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    if path == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }

    Some(PathBuf::from(path))
}
