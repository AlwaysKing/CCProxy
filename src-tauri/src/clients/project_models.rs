// 项目级默认模型：从客户端历史发现工作目录，校验映射，并且只写客户端的顶层 model。

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDateTime};
use serde::Serialize;
use serde_json::Value;

use crate::clients::apply::rewrite_codex_model;
use crate::clients::json::set_object_values;
use crate::clients::write::write_if_changed;
use crate::config::AppConfig;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSources {
    pub codex: bool,
    pub claude: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProject {
    pub path: String,
    pub sources: ProjectSources,
    pub last_activity_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListItem {
    pub id: String,
    pub path: String,
    pub codex_model: Option<String>,
    pub claude_model: Option<String>,
    pub manual: bool,
    pub hidden: bool,
    pub sources: ProjectSources,
    pub last_activity_ms: Option<i64>,
    pub missing: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelCandidates {
    pub codex: Vec<String>,
    pub claude: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSyncState {
    Ok,
    SyncDisabled,
    BindingInvalid,
    ProjectMissing,
    ConfigUnreadable,
    WriteFailed,
    RestartRequired,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSyncResult {
    pub status: ProjectSyncState,
    pub file: Option<String>,
    pub changed: bool,
    pub message: Option<String>,
}

impl ProjectSyncResult {
    fn status(status: ProjectSyncState, message: impl Into<String>) -> Self {
        Self {
            status,
            file: None,
            changed: false,
            message: Some(message.into()),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

fn timestamp_ms(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => {
            let raw = number.as_i64()?;
            Some(if raw.abs() < 10_000_000_000 {
                raw.saturating_mul(1000)
            } else {
                raw
            })
        }
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|value| value.timestamp_millis())
            .or_else(|| {
                NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()
                    .map(|value| value.and_utc().timestamp_millis())
            }),
        _ => None,
    }
}

fn canonical_project(path: &str) -> Option<String> {
    let path = Path::new(path.trim());
    if path.is_dir() {
        fs::canonicalize(path)
            .ok()
            .map(|value| value.to_string_lossy().to_string())
    } else {
        None
    }
}

fn visit_jsonl_files(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_jsonl_files(&path, visit);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            visit(&path);
        }
    }
}

#[derive(Default)]
struct DiscoveryBuilder {
    projects: BTreeMap<String, DiscoveredProject>,
}

impl DiscoveryBuilder {
    fn insert(&mut self, raw_path: &str, codex: bool, claude: bool, activity_ms: Option<i64>) {
        let Some(path) = canonical_project(raw_path) else {
            return;
        };
        let project = self
            .projects
            .entry(path.clone())
            .or_insert_with(|| DiscoveredProject {
                path,
                sources: ProjectSources::default(),
                last_activity_ms: None,
            });
        project.sources.codex |= codex;
        project.sources.claude |= claude;
        project.last_activity_ms = match (project.last_activity_ms, activity_ms) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
    }

    fn finish(self) -> Vec<DiscoveredProject> {
        let mut projects: Vec<DiscoveredProject> = self.projects.into_values().collect();
        projects.sort_by(|left, right| {
            right
                .last_activity_ms
                .unwrap_or(0)
                .cmp(&left.last_activity_ms.unwrap_or(0))
                .then_with(|| left.path.cmp(&right.path))
        });
        projects
    }
}

fn discover_codex(root: &Path, builder: &mut DiscoveryBuilder) {
    visit_jsonl_files(&root.join("sessions"), &mut |path| {
        let Ok(file) = fs::File::open(path) else {
            return;
        };
        for line in BufReader::new(file).lines().take(64).map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if event.get("type").and_then(Value::as_str) != Some("session_meta") {
                continue;
            }
            if let Some(cwd) = event
                .get("payload")
                .and_then(|payload| payload.get("cwd"))
                .and_then(Value::as_str)
            {
                builder.insert(cwd, true, false, file_mtime_ms(path));
            }
            break;
        }
    });
}

fn discover_claude(root: &Path, builder: &mut DiscoveryBuilder) {
    if let Ok(file) = fs::File::open(root.join("history.jsonl")) {
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(project) = event.get("project").and_then(Value::as_str) else {
                continue;
            };
            let activity = timestamp_ms(event.get("timestamp"));
            builder.insert(project, false, true, activity);
        }
    }

    visit_jsonl_files(&root.join("projects"), &mut |path| {
        let Ok(file) = fs::File::open(path) else {
            return;
        };
        let fallback = file_mtime_ms(path);
        for line in BufReader::new(file).lines().take(128).map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(cwd) = event.get("cwd").and_then(Value::as_str) else {
                continue;
            };
            builder.insert(
                cwd,
                false,
                true,
                timestamp_ms(event.get("timestamp")).or(fallback),
            );
            break;
        }
    });
}

fn discover_from_roots(codex_root: &Path, claude_root: &Path) -> Vec<DiscoveredProject> {
    let mut builder = DiscoveryBuilder::default();
    discover_codex(codex_root, &mut builder);
    discover_claude(claude_root, &mut builder);
    builder.finish()
}

pub fn discover_projects(home: &Path) -> Vec<DiscoveredProject> {
    discover_from_roots(&home.join(".codex"), &home.join(".claude"))
}

pub fn discover_default_projects() -> Vec<DiscoveredProject> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let codex_root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let claude_root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    discover_from_roots(&codex_root, &claude_root)
}

fn merge_key(path: &str) -> String {
    canonical_project(path).unwrap_or_else(|| path.trim().to_string())
}

pub fn merge_project_list(
    config: &AppConfig,
    discovered: Vec<DiscoveredProject>,
) -> Vec<ProjectListItem> {
    let mut items: BTreeMap<String, ProjectListItem> = BTreeMap::new();
    for project in &config.settings.project_model_projects {
        let key = merge_key(&project.path);
        items.insert(
            key,
            ProjectListItem {
                id: project.id.clone(),
                path: project.path.clone(),
                codex_model: project.codex_model.clone(),
                claude_model: project.claude_model.clone(),
                manual: project.manual,
                hidden: project.hidden,
                sources: ProjectSources::default(),
                last_activity_ms: None,
                missing: !Path::new(&project.path).is_dir(),
            },
        );
    }
    for project in discovered {
        let key = merge_key(&project.path);
        if let Some(existing) = items.get_mut(&key) {
            existing.sources.codex |= project.sources.codex;
            existing.sources.claude |= project.sources.claude;
            existing.last_activity_ms = match (existing.last_activity_ms, project.last_activity_ms)
            {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
            existing.missing = !Path::new(&existing.path).is_dir();
        } else {
            items.insert(
                key,
                ProjectListItem {
                    id: format!("auto:{}", project.path),
                    path: project.path,
                    codex_model: None,
                    claude_model: None,
                    manual: false,
                    hidden: false,
                    sources: project.sources,
                    last_activity_ms: project.last_activity_ms,
                    missing: false,
                },
            );
        }
    }

    let mut items: Vec<ProjectListItem> = items.into_values().collect();
    items.sort_by(|left, right| {
        let left_bound = left.codex_model.is_some() || left.claude_model.is_some();
        let right_bound = right.codex_model.is_some() || right.claude_model.is_some();
        right_bound
            .cmp(&left_bound)
            .then_with(|| {
                right
                    .last_activity_ms
                    .unwrap_or(0)
                    .cmp(&left.last_activity_ms.unwrap_or(0))
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    items
}

fn protocol_candidates(config: &AppConfig, protocol_id: &str) -> Vec<String> {
    let Some(protocol) = config.protocol(protocol_id) else {
        return Vec::new();
    };
    if !protocol.enabled {
        return Vec::new();
    }
    let mut names = Vec::new();
    for mapping in &protocol.mappings {
        if !mapping.enabled
            || mapping.client_model.is_empty()
            || mapping.client_model.contains('*')
            || names.iter().any(|name| name == &mapping.client_model)
        {
            continue;
        }
        let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.id == mapping.provider_id && provider.enabled)
        else {
            continue;
        };
        if !provider
            .model_groups
            .iter()
            .any(|group| group.id == mapping.model_group_id)
        {
            continue;
        }
        names.push(mapping.client_model.clone());
    }
    names
}

pub fn project_model_candidates(config: &AppConfig) -> ProjectModelCandidates {
    ProjectModelCandidates {
        codex: protocol_candidates(config, "openai_responses"),
        claude: protocol_candidates(config, "anthropic"),
    }
}

fn binding_is_valid(config: &AppConfig, client: &str, model: &str) -> bool {
    let protocol_id = match client {
        "codex" => "openai_responses",
        "claude" => "anthropic",
        _ => return false,
    };
    let Some(protocol) = config.protocol(protocol_id) else {
        return false;
    };
    if !protocol.enabled {
        return false;
    }
    protocol.mappings.iter().any(|mapping| {
        if !mapping.enabled || mapping.client_model != model {
            return false;
        }
        let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.id == mapping.provider_id && provider.enabled)
        else {
            return false;
        };
        provider
            .model_groups
            .iter()
            .any(|group| group.id == mapping.model_group_id)
    })
}

fn project_config(client: &str, root: &Path) -> (PathBuf, &'static str) {
    match client {
        "codex" => (root.join(".codex").join("config.toml"), ""),
        _ => (root.join(".claude").join("settings.local.json"), "{}\n"),
    }
}

pub fn apply_project_model(
    config: &AppConfig,
    project_path: &str,
    client: &str,
    model: &str,
) -> ProjectSyncResult {
    if client != "codex" && client != "claude" {
        return ProjectSyncResult::status(ProjectSyncState::BindingInvalid, "未知的客户端类型");
    }
    if !binding_is_valid(config, client, model) {
        return ProjectSyncResult::status(
            ProjectSyncState::BindingInvalid,
            format!("模型 {model} 不在可路由的精确映射中"),
        );
    }
    let root = Path::new(project_path);
    if !root.is_dir() {
        return ProjectSyncResult::status(ProjectSyncState::ProjectMissing, "项目目录不存在");
    }

    let (file_path, empty) = project_config(client, root);
    let original = match fs::read_to_string(&file_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => empty.to_string(),
        Err(error) => {
            return ProjectSyncResult::status(
                ProjectSyncState::ConfigUnreadable,
                format!("读取 {} 失败：{error}", file_path.to_string_lossy()),
            )
        }
    };
    let next = if client == "codex" {
        match rewrite_codex_model(&original, model) {
            Ok(text) => (text.clone(), text != original),
            Err(error) => {
                return ProjectSyncResult::status(ProjectSyncState::ConfigUnreadable, error)
            }
        }
    } else {
        match set_object_values(
            &original,
            "",
            &[("model".to_string(), Value::String(model.to_string()))],
        ) {
            Ok(edit) => (edit.text, edit.changed),
            Err(error) => {
                return ProjectSyncResult::status(ProjectSyncState::ConfigUnreadable, error)
            }
        }
    };

    if next.1 {
        if let Some(parent) = file_path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return ProjectSyncResult::status(
                    ProjectSyncState::WriteFailed,
                    format!("创建 {} 失败：{error}", parent.to_string_lossy()),
                );
            }
        }
    }
    match write_if_changed(&file_path, &next.0, next.1) {
        Ok(result) => ProjectSyncResult {
            status: ProjectSyncState::Ok,
            file: Some(result.file),
            changed: result.changed,
            message: None,
        },
        Err(error) => ProjectSyncResult::status(ProjectSyncState::WriteFailed, error),
    }
}

pub fn project_sync_timestamp_ms() -> i64 {
    now_ms()
}
