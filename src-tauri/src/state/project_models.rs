// 项目模型状态层：候选缓存、按协议的绑定事务以及文件同步。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use super::{AppState, ProjectSyncStatusSnapshot};
use crate::clients::project_models::{
    apply_project_model, claude_settings_root, codex_project_trust, discover_default_projects,
    discovery_candidates, project_binding_list, DiscoveredProject, ProjectBindingListItem,
    ProjectSyncResult, ProjectSyncState,
};
use crate::config::{create_config_id, AppConfig, ProjectModelBinding};
use crate::logger;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelRow {
    #[serde(flatten)]
    pub binding: ProjectBindingListItem,
    pub status: Option<ProjectSyncStatusSnapshot>,
    pub codex_trusted: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelsView {
    pub protocol: String,
    pub client: String,
    pub mapping_id: String,
    pub model: String,
    pub title: String,
    pub projects: Vec<ProjectModelRow>,
    pub directory_candidates: Vec<String>,
    pub codex_daemon: Option<crate::clients::daemon::CodexDaemon>,
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn client_for_protocol(protocol: &str) -> Option<(&'static str, &'static str)> {
    match protocol {
        "anthropic" => Some(("claude", "Claude")),
        "openai_responses" => Some(("codex", "Codex")),
        _ => None,
    }
}

fn mapping_model(config: &AppConfig, protocol: &str, mapping_id: &str) -> Option<String> {
    config
        .protocol(protocol)?
        .mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id && !mapping.client_model.contains('*'))
        .map(|mapping| mapping.client_model.clone())
}

fn same_project_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => Path::new(left) == Path::new(right),
    }
}

fn claude_root(path: &str) -> PathBuf {
    let root = claude_settings_root(path);
    std::fs::canonicalize(&root).unwrap_or(root)
}

impl AppState {
    fn codex_daemon(&self) -> Option<crate::clients::daemon::CodexDaemon> {
        self.handle()?;
        Some(crate::clients::daemon::inspect(
            &codex_home().to_string_lossy(),
        ))
    }

    fn record_project_sync(
        &self,
        binding_id: &str,
        client: &str,
        result: ProjectSyncResult,
        daemon_pid: Option<u32>,
        codex_trusted: Option<bool>,
    ) {
        let mut status = result.status.clone();
        let mut message = result.message.clone();
        if status == ProjectSyncState::Ok && client == "codex" {
            if codex_trusted != Some(true) {
                status = ProjectSyncState::RequiresProjectTrust;
                message = Some("Codex 只会加载受信任项目的 config.toml".to_string());
            } else if daemon_pid.is_some() {
                status = ProjectSyncState::RestartRequired;
                message = Some(
                    "Codex app-server daemon 正在运行，重启后才会读取新的项目模型".to_string(),
                );
            }
        }
        if let Ok(mut statuses) = self.project_sync_status.write() {
            statuses.insert(
                binding_id.to_string(),
                ProjectSyncStatusSnapshot {
                    status,
                    message,
                    file: result.file,
                    changed: result.changed,
                    synced_at_ms: crate::now_ms(),
                    daemon_pid,
                },
            );
        }
    }

    pub(super) fn sync_all_project_models_locked(self: &Arc<Self>, config: &AppConfig) {
        let daemon_pid = self
            .codex_daemon()
            .filter(|daemon| daemon.running)
            .and_then(|daemon| daemon.pid);
        let mut claude_roots: HashMap<PathBuf, String> = HashMap::new();
        for binding in &config.settings.project_model_bindings {
            let protocol = if binding.client == "codex" {
                "openai_responses"
            } else {
                "anthropic"
            };
            let Some(model) = mapping_model(config, protocol, &binding.mapping_id) else {
                self.record_project_sync(
                    &binding.id,
                    &binding.client,
                    ProjectSyncResult {
                        status: ProjectSyncState::BindingInvalid,
                        file: None,
                        changed: false,
                        message: Some("项目绑定引用的映射已删除、失效或不是精确模型".to_string()),
                    },
                    None,
                    None,
                );
                continue;
            };
            if binding.client == "claude" {
                let root = claude_root(&binding.path);
                if let Some(owner) = claude_roots.get(&root) {
                    self.record_project_sync(
                        &binding.id,
                        "claude",
                        ProjectSyncResult {
                            status: ProjectSyncState::BindingInvalid,
                            file: None,
                            changed: false,
                            message: Some(format!(
                                "与项目 {owner} 共用同一个 Claude settings.local.json，已跳过"
                            )),
                        },
                        None,
                        None,
                    );
                    continue;
                }
                claude_roots.insert(root, binding.path.clone());
            }
            let codex_trusted = (binding.client == "codex" && self.handle().is_some())
                .then(|| codex_project_trust(&binding.path, &codex_home()))
                .flatten();
            let result = apply_project_model(config, &binding.path, &binding.client, &model);
            if result.status != ProjectSyncState::Ok {
                logger::warn(
                    "[ProjectModels] 同步未完成",
                    Some(&json!({
                        "project": binding.path,
                        "client": binding.client,
                        "status": result.status,
                        "message": result.message
                    })),
                );
            }
            self.record_project_sync(
                &binding.id,
                &binding.client,
                result,
                (binding.client == "codex").then_some(daemon_pid).flatten(),
                codex_trusted,
            );
        }
        if let Ok(mut statuses) = self.project_sync_status.write() {
            let ids: HashSet<&str> = config
                .settings
                .project_model_bindings
                .iter()
                .map(|binding| binding.id.as_str())
                .collect();
            statuses.retain(|id, _| ids.contains(id.as_str()));
        }
        self.emit(
            "project-models-changed",
            &json!({ "syncedAt": crate::now_ms() }),
        );
    }

    fn refresh_restart_required(&self) {
        let Some(daemon) = self.codex_daemon() else {
            return;
        };
        if let Ok(mut statuses) = self.project_sync_status.write() {
            for status in statuses.values_mut() {
                if status.status == ProjectSyncState::RestartRequired
                    && (!daemon.running || daemon.pid != status.daemon_pid)
                {
                    status.status = ProjectSyncState::Ok;
                    status.message = None;
                    status.daemon_pid = None;
                }
            }
        }
    }

    pub async fn ensure_project_candidates(
        self: &Arc<Self>,
        refresh: bool,
    ) -> Vec<DiscoveredProject> {
        if !refresh {
            if let Ok(cache) = self.project_candidates.read() {
                if let Some(projects) = cache.as_ref() {
                    return projects.clone();
                }
            }
        }
        let _guard = self.project_candidates_scan.lock().await;
        if !refresh {
            if let Ok(cache) = self.project_candidates.read() {
                if let Some(projects) = cache.as_ref() {
                    return projects.clone();
                }
            }
        }
        let projects = tokio::task::spawn_blocking(discover_default_projects)
            .await
            .unwrap_or_default();
        if let Ok(mut cache) = self.project_candidates.write() {
            *cache = Some(projects.clone());
        }
        projects
    }

    pub fn prime_project_candidates(self: &Arc<Self>) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            state.ensure_project_candidates(true).await;
        });
    }

    pub fn cache_project_candidates(&self, projects: Vec<DiscoveredProject>) {
        if let Ok(mut cache) = self.project_candidates.write() {
            *cache = Some(projects);
        }
    }

    pub async fn project_models_view(
        self: &Arc<Self>,
        protocol: &str,
        mapping_id: &str,
        refresh: bool,
    ) -> Result<ProjectModelsView, String> {
        let (client, title) =
            client_for_protocol(protocol).ok_or_else(|| "该协议不支持项目默认模型".to_string())?;
        self.refresh_restart_required();
        let config = self.config_snapshot();
        let model = mapping_model(&config, protocol, mapping_id)
            .ok_or_else(|| "映射不存在或不是精确模型".to_string())?;
        let discovered = self.ensure_project_candidates(refresh).await;
        let statuses = self
            .project_sync_status
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let projects = project_binding_list(&config, client, mapping_id)
            .into_iter()
            .map(|binding| {
                let codex_trusted = (client == "codex" && self.handle().is_some())
                    .then(|| codex_project_trust(&binding.path, &codex_home()))
                    .flatten();
                let mut status = statuses.get(&binding.id).cloned();
                if codex_trusted == Some(true) {
                    if let Some(current) = status.as_mut() {
                        if current.status == ProjectSyncState::RequiresProjectTrust {
                            current.status = ProjectSyncState::Ok;
                            current.message = None;
                        }
                    }
                }
                ProjectModelRow {
                    codex_trusted,
                    status,
                    binding,
                }
            })
            .collect();
        Ok(ProjectModelsView {
            protocol: protocol.to_string(),
            client: client.to_string(),
            mapping_id: mapping_id.to_string(),
            model: model.clone(),
            title: format!("{title} 项目 · {model}"),
            projects,
            directory_candidates: discovery_candidates(&discovered, client),
            codex_daemon: (client == "codex").then(|| self.codex_daemon()).flatten(),
        })
    }

    fn save_project_draft_locked(self: &Arc<Self>, draft: &AppConfig) -> Result<AppConfig, String> {
        let value = serde_json::to_value(draft).map_err(|error| error.to_string())?;
        self.apply_config_locked(&value).map(|(config, _)| config)
    }

    pub async fn add_project_bindings(
        self: &Arc<Self>,
        protocol: &str,
        mapping_id: &str,
        paths: Vec<String>,
    ) -> Result<(), String> {
        let (client, _) =
            client_for_protocol(protocol).ok_or_else(|| "该协议不支持项目默认模型".to_string())?;
        let candidates = self.ensure_project_candidates(false).await;
        let allowed: HashSet<String> = discovery_candidates(&candidates, client)
            .into_iter()
            .collect();
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        if mapping_model(&draft, protocol, mapping_id).is_none() {
            return Err("映射不存在或不是精确模型".to_string());
        }
        for raw_path in paths {
            let path = std::fs::canonicalize(raw_path.trim())
                .map_err(|error| format!("项目目录不可用：{error}"))?;
            let path = path.to_string_lossy().to_string();
            if !allowed.contains(&path) {
                return Err(format!("{path} 不在当前客户端的会话候选里"));
            }
            if draft
                .settings
                .project_model_bindings
                .iter()
                .any(|binding| binding.client == client && same_project_path(&binding.path, &path))
            {
                continue;
            }
            if client == "claude" {
                let root = claude_root(&path);
                if draft
                    .settings
                    .project_model_bindings
                    .iter()
                    .any(|binding| binding.client == "claude" && claude_root(&binding.path) == root)
                {
                    return Err(format!("{path} 与已有 Claude 项目共用 settings.local.json"));
                }
            }
            draft
                .settings
                .project_model_bindings
                .push(ProjectModelBinding {
                    id: create_config_id("b"),
                    path,
                    client: client.to_string(),
                    mapping_id: mapping_id.to_string(),
                });
        }
        self.save_project_draft_locked(&draft)?;
        Ok(())
    }

    pub fn remove_project_binding(
        self: &Arc<Self>,
        protocol: &str,
        binding_id: &str,
    ) -> Result<(), String> {
        let (client, _) =
            client_for_protocol(protocol).ok_or_else(|| "该协议不支持项目默认模型".to_string())?;
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        draft
            .settings
            .project_model_bindings
            .retain(|binding| !(binding.id == binding_id && binding.client == client));
        self.save_project_draft_locked(&draft)?;
        Ok(())
    }
}
