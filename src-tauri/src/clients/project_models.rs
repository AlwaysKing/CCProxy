// 项目级默认模型：从客户端历史发现工作目录，校验映射，并且只写客户端的顶层 model。

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, NaiveDateTime};
use serde::Serialize;
use serde_json::Value;

use crate::clients::apply::rewrite_codex_model;
use crate::clients::json::set_object_values;
use crate::clients::write::{ensure_write_within, write_if_changed_within};
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

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelCandidate {
    pub mapping_id: String,
    pub model: String,
    pub invalid: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelCandidates {
    pub codex: Vec<ProjectModelCandidate>,
    pub claude: Vec<ProjectModelCandidate>,
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
    RequiresProjectTrust,
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

fn protected_project_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".codex"));
        roots.push(home.join(".claude"));
        roots.push(home);
    }
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        roots.push(PathBuf::from(codex_home));
    }
    if let Some(claude_home) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        roots.push(PathBuf::from(claude_home));
    }
    roots
}

fn is_allowed_project_path(path: &Path) -> bool {
    protected_project_roots()
        .into_iter()
        .all(|root| path != root)
}

fn canonical_project(path: &str) -> Option<String> {
    let path = Path::new(path.trim());
    if path.is_dir() {
        fs::canonicalize(path).ok().and_then(|value| {
            is_allowed_project_path(&value).then(|| value.to_string_lossy().to_string())
        })
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
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            visit_jsonl_files(&path, visit);
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        {
            visit(&path);
        }
    }
}

const MAX_SCAN_LINE_BYTES: u64 = 256 * 1024;

fn limited_lines(path: &Path, max_lines: usize) -> Vec<String> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    for _ in 0..max_lines {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_SCAN_LINE_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .unwrap_or(0);
        if read == 0 {
            break;
        }
        if bytes.len() as u64 > MAX_SCAN_LINE_BYTES {
            // 超长行整行丢掉；若这次没读到最后，再消费到下一个换行，避免把半行当 JSON。
            if !bytes.ends_with(b"\n") {
                let mut discarded = Vec::new();
                let _ = reader.read_until(b'\n', &mut discarded);
            }
            continue;
        }
        if let Ok(line) = String::from_utf8(bytes) {
            lines.push(line);
        }
    }
    lines
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
        for line in limited_lines(path, 8) {
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
    let history = root.join("history.jsonl");
    if history.exists() {
        for line in limited_lines(&history, usize::MAX) {
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
        let fallback = file_mtime_ms(path);
        for line in limited_lines(path, 32) {
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

fn protocol_candidates(config: &AppConfig, protocol_id: &str) -> Vec<ProjectModelCandidate> {
    let Some(protocol) = config
        .protocol(protocol_id)
        .filter(|protocol| protocol.enabled)
    else {
        return Vec::new();
    };
    let usable = protocol
        .mappings
        .iter()
        .filter(|mapping| {
            if !mapping.enabled {
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
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for (index, mapping) in usable.iter().enumerate() {
        if mapping.client_model.is_empty() || mapping.client_model.contains('*') {
            continue;
        }
        if usable[..index]
            .iter()
            .any(|earlier| crate::proxy::router::match_mapping(earlier, &mapping.client_model))
        {
            continue;
        }
        candidates.push(ProjectModelCandidate {
            mapping_id: mapping.id.clone(),
            model: mapping.client_model.clone(),
            invalid: false,
        });
    }
    candidates
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

fn git_path(root: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

pub fn claude_settings_root(project_path: &str) -> PathBuf {
    let root = fs::canonicalize(project_path).unwrap_or_else(|_| PathBuf::from(project_path));
    if let Some(common_dir) = git_path(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        if common_dir.file_name().and_then(|value| value.to_str()) == Some(".git") {
            if let Some(main_checkout) = common_dir.parent() {
                return main_checkout.to_path_buf();
            }
        }
    }
    git_path(&root, &["rev-parse", "--show-toplevel"]).unwrap_or(root)
}

pub fn codex_project_trust(project_path: &str, codex_home: &Path) -> Option<bool> {
    let text = fs::read_to_string(codex_home.join("config.toml")).ok()?;
    let document = text.parse::<toml_edit::DocumentMut>().ok()?;
    let projects = document.get("projects")?.as_table_like()?;
    let canonical = fs::canonicalize(project_path)
        .unwrap_or_else(|_| PathBuf::from(project_path))
        .to_string_lossy()
        .to_string();
    let project = projects
        .get(&canonical)
        .or_else(|| projects.get(project_path.trim()))?;
    match project.as_table_like()?.get("trust_level")?.as_str()? {
        "trusted" => Some(true),
        "untrusted" => Some(false),
        _ => None,
    }
}

fn project_config(client: &str, root: &Path) -> (PathBuf, &'static str, PathBuf) {
    match client {
        "codex" => (
            root.join(".codex").join("config.toml"),
            "",
            root.to_path_buf(),
        ),
        _ => {
            let claude_root = claude_settings_root(&root.to_string_lossy());
            (
                claude_root.join(".claude").join("settings.local.json"),
                "{}\n",
                claude_root,
            )
        }
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
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !is_allowed_project_path(&canonical_root) {
        return ProjectSyncResult::status(
            ProjectSyncState::BindingInvalid,
            "项目目录不能是根目录、用户主目录或客户端配置目录",
        );
    }

    let (file_path, empty, write_root) = project_config(client, &canonical_root);
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

    if let Err(error) = ensure_write_within(&write_root, &file_path) {
        return ProjectSyncResult::status(ProjectSyncState::ConfigUnreadable, error);
    }
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
    match write_if_changed_within(&write_root, &file_path, &next.0, next.1) {
        Ok(result) => ProjectSyncResult {
            status: ProjectSyncState::Ok,
            file: Some(result.file),
            changed: result.changed,
            message: None,
        },
        Err(error) => ProjectSyncResult::status(ProjectSyncState::WriteFailed, error),
    }
}
