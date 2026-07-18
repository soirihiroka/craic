use std::collections::{BTreeMap, btree_map::Entry};

use crate::system::WorkspacePath;
use crate::system::capabilities::docker::DockerAccess;
use serde_json::{Map, Value};

pub const INDIVIDUAL_CONTAINERS_GROUP: &str = "Individual Containers";
pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
pub const COMPOSE_WORKING_DIR_LABEL: &str = "com.docker.compose.project.working_dir";
pub const COMPOSE_CONFIG_FILES_LABEL: &str = "com.docker.compose.project.config_files";
pub const COMPOSE_ENVIRONMENT_FILE_LABEL: &str = "com.docker.compose.project.environment_file";
pub const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerInventory {
    pub groups: Vec<ContainerGroup>,
}

impl ContainerInventory {
    pub fn container_count(&self) -> usize {
        self.groups.iter().map(|group| group.containers.len()).sum()
    }

    pub fn group_by_key(&self, key: &str) -> Option<&ContainerGroup> {
        self.groups.iter().find(|group| group.key == key)
    }

    pub fn container_by_id(&self, id: &str) -> Option<&ContainerSummary> {
        self.groups
            .iter()
            .flat_map(|group| &group.containers)
            .find(|container| container.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerGroup {
    pub key: String,
    pub name: String,
    pub title: String,
    pub kind: ContainerGroupKind,
    pub compose: Option<ComposeProject>,
    pub containers: Vec<ContainerSummary>,
}

impl ContainerGroup {
    pub fn is_compose(&self) -> bool {
        self.compose.is_some()
    }

    pub fn compose_metadata(&self) -> Option<&ComposeProject> {
        self.compose.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerGroupKind {
    Compose(ComposeProject),
    Individual,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComposeProject {
    pub project: String,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub config_files_raw: Option<String>,
    pub environment_file: Option<String>,
}

impl ComposeProject {
    fn from_labels(labels: &BTreeMap<String, String>) -> Option<Self> {
        let project = label_value(labels, COMPOSE_PROJECT_LABEL)?;
        let config_files_raw = label_value(labels, COMPOSE_CONFIG_FILES_LABEL);
        Some(Self {
            project,
            working_dir: label_value(labels, COMPOSE_WORKING_DIR_LABEL),
            config_files: config_files_raw
                .as_deref()
                .map(split_docker_list)
                .unwrap_or_default(),
            config_files_raw,
            environment_file: label_value(labels, COMPOSE_ENVIRONMENT_FILE_LABEL),
        })
    }

    fn group_key(&self) -> String {
        let working_dir = self.working_dir.as_ref().cloned().unwrap_or_default();
        format!(
            "compose:{}:{}:{}:{}",
            self.project,
            working_dir,
            self.config_files.join(","),
            self.environment_file.as_deref().unwrap_or_default()
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    pub created_at: String,
    pub created: String,
    pub running_for: String,
    pub ports: String,
    pub ports_raw: String,
    pub status: String,
    pub size: String,
    pub labels: BTreeMap<String, String>,
    pub labels_raw: String,
    pub networks: Vec<String>,
    pub networks_raw: String,
    pub mounts: Vec<String>,
    pub mounts_raw: String,
    pub state: ContainerState,
    pub service: Option<String>,
    pub compose: Option<ComposeProject>,
}

pub type DockerContainer = ContainerSummary;
pub type ContainerState = String;

impl ContainerSummary {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }

    pub fn action_enablement(&self) -> ContainerActionEnablement {
        ContainerActionEnablement {
            start: can_start(&self.state),
            stop: can_stop(&self.state),
            restart: can_restart(&self.state),
            remove: can_remove(&self.state),
        }
    }

    pub fn can_start(&self) -> bool {
        can_start(&self.state)
    }

    pub fn can_stop(&self) -> bool {
        can_stop(&self.state)
    }

    pub fn can_restart(&self) -> bool {
        can_restart(&self.state)
    }

    pub fn can_remove(&self) -> bool {
        can_remove(&self.state)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Remove,
}

impl ContainerAction {
    fn docker_arg(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Remove => "rm",
        }
    }

    fn success_message(self, container: &str) -> String {
        let verb = match self {
            Self::Start => "Started",
            Self::Stop => "Stopped",
            Self::Restart => "Restarted",
            Self::Remove => "Removed",
        };
        format!("{verb} container {}.", short_id(container))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeAction {
    Start,
    Stop,
    Restart,
    Down,
}

impl ComposeAction {
    fn docker_arg(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Down => "down",
        }
    }

    fn success_message(self, compose: &ComposeProject) -> String {
        let verb = match self {
            Self::Start => "Started",
            Self::Stop => "Stopped",
            Self::Restart => "Restarted",
            Self::Down => "Stopped and removed",
        };
        format!("{verb} Compose project {}.", compose.project)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContainerActionEnablement {
    pub start: bool,
    pub stop: bool,
    pub restart: bool,
    pub remove: bool,
}

impl ContainerActionEnablement {
    pub fn is_enabled(self, action: ContainerAction) -> bool {
        match action {
            ContainerAction::Start => self.start,
            ContainerAction::Stop => self.stop,
            ContainerAction::Restart => self.restart,
            ContainerAction::Remove => self.remove,
        }
    }
}

pub fn list_inventory(access: &dyn DockerAccess) -> Result<ContainerInventory, String> {
    list_containers(access).map(|containers| ContainerInventory {
        groups: group_containers(containers),
    })
}

pub fn list_containers(access: &dyn DockerAccess) -> Result<Vec<ContainerSummary>, String> {
    log::debug!("loading docker containers");
    let output = access.run_docker(
        &strings(["ps", "-a", "--no-trunc", "--format", "{{json .}}"]),
        None,
    )?;
    let text = String::from_utf8_lossy(&output);
    let containers = parse_container_lines(&text)?;
    log::debug!("loaded docker containers count={}", containers.len());
    Ok(containers)
}

pub fn group_containers(containers: Vec<ContainerSummary>) -> Vec<ContainerGroup> {
    let mut compose_groups = BTreeMap::<String, ContainerGroup>::new();
    let mut individual = Vec::new();

    for container in containers {
        let Some(compose) = container.compose.clone() else {
            individual.push(container);
            continue;
        };
        let key = compose.group_key();

        match compose_groups.entry(key.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().containers.push(container);
            }
            Entry::Vacant(entry) => {
                let title = compose.project.clone();
                entry.insert(ContainerGroup {
                    key,
                    name: title.clone(),
                    title,
                    kind: ContainerGroupKind::Compose(compose.clone()),
                    compose: Some(compose),
                    containers: vec![container],
                });
            }
        }
    }

    let mut groups = compose_groups.into_values().collect::<Vec<_>>();
    if !individual.is_empty() {
        groups.push(ContainerGroup {
            key: "individual-containers".to_string(),
            name: INDIVIDUAL_CONTAINERS_GROUP.to_string(),
            title: INDIVIDUAL_CONTAINERS_GROUP.to_string(),
            kind: ContainerGroupKind::Individual,
            compose: None,
            containers: individual,
        });
    }
    log::debug!("grouped docker containers group_count={}", groups.len());
    groups
}

pub fn inspect_container(access: &dyn DockerAccess, container: &str) -> Result<String, String> {
    let container = container.trim();
    if container.is_empty() {
        return Err("Choose a Docker container to inspect.".to_string());
    }

    log::debug!("inspecting docker container target={container}");
    let output = access.run_docker(&strings(["inspect", container]), None)?;
    let value = serde_json::from_slice::<Value>(&output).map_err(|err| {
        let text = String::from_utf8_lossy(&output);
        format!(
            "Docker inspect returned invalid JSON for {container}: {err}. Body: {}",
            preview_text(&text)
        )
    })?;
    let formatted = serde_json::to_string_pretty(&value)
        .map_err(|err| format!("Failed to format docker inspect output for {container}: {err}"))?;
    log::debug!(
        "inspected docker container target={} output_bytes={}",
        container,
        formatted.len()
    );
    Ok(formatted)
}

pub fn run_container_action(
    access: &dyn DockerAccess,
    container: &str,
    action: ContainerAction,
) -> Result<String, String> {
    let container = container.trim();
    if container.is_empty() {
        return Err("Choose a Docker container.".to_string());
    }

    log::debug!("running docker container action action={action:?} target={container}");
    access.run_docker(&strings([action.docker_arg(), container]), None)?;

    log::debug!("completed docker container action action={action:?} target={container}");
    Ok(action.success_message(container))
}

pub fn run_compose_action(
    access: &dyn DockerAccess,
    compose: &ComposeProject,
    action: ComposeAction,
) -> Result<String, String> {
    let Some(working_dir) = &compose.working_dir else {
        return Err(format!(
            "Compose project '{}' does not include the {} label.",
            compose.project, COMPOSE_WORKING_DIR_LABEL
        ));
    };

    log::debug!(
        "running docker compose action action={action:?} project={} working_dir={}",
        compose.project,
        working_dir
    );
    let working_dir = WorkspacePath::from_absolute(working_dir.clone());
    let success_message = match action {
        ComposeAction::Restart => {
            if run_compose_restart_action(access, compose, &working_dir)? {
                format!("Started Compose project {}.", compose.project)
            } else {
                ComposeAction::Restart.success_message(compose)
            }
        }
        _ => {
            access.run_docker(&compose_action_args(compose, action), Some(&working_dir))?;
            action.success_message(compose)
        }
    };

    log::debug!(
        "completed docker compose action action={action:?} project={}",
        compose.project
    );
    Ok(success_message)
}

fn run_compose_restart_action(
    access: &dyn DockerAccess,
    compose: &ComposeProject,
    working_dir: &WorkspacePath,
) -> Result<bool, String> {
    let restart_result = access.run_docker(
        &compose_action_args(compose, ComposeAction::Restart),
        Some(working_dir),
    );

    let started_instead = match restart_result {
        Ok(_) => false,
        Err(error) if restart_action_should_fallback_to_up(&error) => {
            log::info!(
                "docker compose restart fallback action=up project={}",
                compose.project
            );
            true
        }
        Err(error) => return Err(error),
    };

    if !started_instead {
        log::debug!(
            "docker compose restart ensure-running action=up project={}",
            compose.project
        );
    }

    access.run_docker(&compose_up_args(compose), Some(working_dir))?;
    Ok(started_instead)
}

fn compose_action_args(compose: &ComposeProject, action: ComposeAction) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    append_compose_args(&mut args, compose);
    args.push(action.docker_arg().to_string());
    args
}

fn compose_up_args(compose: &ComposeProject) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    append_compose_args(&mut args, compose);
    args.push("up".to_string());
    args.push("-d".to_string());
    args.push("--build".to_string());
    args
}

fn restart_action_should_fallback_to_up(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such container")
        || error.contains("no such service")
        || error.contains("has no container")
        || error.contains("no container to restart")
}

pub fn state_is_running(state: impl AsRef<str>) -> bool {
    matches!(
        normalized_state(state.as_ref()).as_str(),
        "running" | "restarting"
    )
}

pub fn can_start(state: impl AsRef<str>) -> bool {
    matches!(
        normalized_state(state.as_ref()).as_str(),
        "created" | "exited"
    )
}

pub fn can_stop(state: impl AsRef<str>) -> bool {
    matches!(
        normalized_state(state.as_ref()).as_str(),
        "running" | "restarting"
    )
}

pub fn can_restart(state: impl AsRef<str>) -> bool {
    normalized_state(state.as_ref()) == "running"
}

pub fn can_remove(state: impl AsRef<str>) -> bool {
    matches!(
        normalized_state(state.as_ref()).as_str(),
        "created" | "exited" | "dead"
    )
}

fn parse_container_lines(output: &str) -> Result<Vec<ContainerSummary>, String> {
    let mut containers = Vec::new();

    for (index, line) in output.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let value = serde_json::from_str::<Value>(line).map_err(|err| {
            format!(
                "Failed to parse docker ps JSON line {}: {err}. Line: {}",
                index + 1,
                preview_text(line)
            )
        })?;
        let Value::Object(object) = value else {
            return Err(format!(
                "docker ps JSON line {} was not an object: {}",
                index + 1,
                preview_text(line)
            ));
        };
        containers.push(container_from_object(object, index + 1)?);
    }

    Ok(containers)
}

fn container_from_object(
    object: Map<String, Value>,
    line_number: usize,
) -> Result<ContainerSummary, String> {
    let id = field_string(&object, "ID").trim().to_string();
    if id.is_empty() {
        return Err(format!(
            "docker ps JSON line {line_number} did not include ID"
        ));
    }

    let labels_raw = field_string(&object, "Labels");
    let labels = parse_labels_value(object.get("Labels"));
    let compose = ComposeProject::from_labels(&labels);
    let name = field_string(&object, "Names");
    let name = if name.trim().is_empty() {
        short_id(&id).to_string()
    } else {
        name
    };
    let ports_raw = field_string(&object, "Ports");
    let networks_raw = field_string(&object, "Networks");
    let mounts_raw = field_string(&object, "Mounts");
    let created_at = field_string(&object, "CreatedAt");
    let service = label_value(&labels, COMPOSE_SERVICE_LABEL);

    Ok(ContainerSummary {
        id,
        name,
        image: field_string(&object, "Image"),
        command: field_string(&object, "Command"),
        created_at: created_at.clone(),
        created: created_at,
        running_for: field_string(&object, "RunningFor"),
        ports: ports_raw.clone(),
        ports_raw,
        status: field_string(&object, "Status"),
        size: field_string(&object, "Size"),
        labels,
        labels_raw,
        networks: split_docker_list(&networks_raw),
        networks_raw,
        mounts: split_docker_list(&mounts_raw),
        mounts_raw,
        state: docker_state(&object),
        service,
        compose,
    })
}

fn docker_state(object: &Map<String, Value>) -> ContainerState {
    let state = field_string(object, "State");
    if !state.trim().is_empty() {
        return normalized_or_unknown(&state);
    }

    let state = field_string(object, "Status")
        .split_whitespace()
        .next()
        .map(normalized_state)
        .filter(|state| !state.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    normalized_or_unknown(&state)
}

fn field_string(object: &Map<String, Value>, key: &str) -> String {
    object.get(key).map(value_to_string).unwrap_or_default()
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn parse_labels_value(value: Option<&Value>) -> BTreeMap<String, String> {
    match value {
        Some(Value::Object(object)) => object
            .iter()
            .filter_map(|(key, value)| {
                let key = key.trim();
                if key.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value_to_string(value)))
                }
            })
            .collect(),
        Some(Value::String(value)) => parse_label_string(value),
        Some(value) => parse_label_string(&value_to_string(value)),
        None => BTreeMap::new(),
    }
}

fn parse_label_string(labels: &str) -> BTreeMap<String, String> {
    let trimmed_labels = labels.trim();
    if trimmed_labels.is_empty() {
        return BTreeMap::new();
    }

    if trimmed_labels.starts_with('{') {
        if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(trimmed_labels) {
            return object
                .iter()
                .filter_map(|(key, value)| {
                    let key = key.trim();
                    if key.is_empty() {
                        None
                    } else {
                        Some((key.to_string(), value_to_string(value)))
                    }
                })
                .collect();
        }
    }

    let mut parsed = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for segment in labels.split(',') {
        if let Some((key, value)) = segment.split_once('=') {
            let key = key.trim();
            if looks_like_label_key(key) {
                if let Some(previous_key) = current_key.replace(key.to_string()) {
                    parsed.insert(previous_key, std::mem::take(&mut current_value));
                }
                current_value.push_str(value);
                continue;
            }
        }

        if current_key.is_some() {
            if !current_value.is_empty() {
                current_value.push(',');
            }
            current_value.push_str(segment);
            continue;
        }

        let key = segment.trim();
        if looks_like_label_key(key) {
            parsed.entry(key.to_string()).or_default();
        }
    }

    if let Some(key) = current_key {
        parsed.insert(key, current_value);
    }

    parsed
}

fn looks_like_label_key(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/'))
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
}

fn label_value(labels: &BTreeMap<String, String>, key: &str) -> Option<String> {
    labels
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn split_docker_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn strings<'a>(args: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    args.into_iter().map(ToString::to_string).collect()
}

pub fn compose_args(compose: &ComposeProject, args: &[&str]) -> Vec<String> {
    let mut command_args = vec!["compose".to_string()];
    append_compose_args(&mut command_args, compose);
    command_args.extend(args.iter().map(|arg| (*arg).to_string()));
    command_args
}

fn append_compose_args(command: &mut Vec<String>, compose: &ComposeProject) {
    for file in &compose.config_files {
        command.push("-f".to_string());
        command.push(file.clone());
    }
    if let Some(environment_file) = &compose.environment_file {
        command.push("--env-file".to_string());
        command.push(environment_file.clone());
    }
    command.push("-p".to_string());
    command.push(compose.project.clone());
}

fn normalized_state(state: &str) -> String {
    state.trim().to_ascii_lowercase()
}

fn normalized_or_unknown(state: &str) -> String {
    let state = normalized_state(state);
    if state.is_empty() {
        "unknown".to_string()
    } else {
        state
    }
}

fn preview_text(value: &str) -> String {
    const MAX_CHARS: usize = 500;
    let value = value.trim();
    if value.chars().count() <= MAX_CHARS {
        value.to_string()
    } else {
        let preview = value.chars().take(MAX_CHARS).collect::<String>();
        format!("{preview}...")
    }
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}
