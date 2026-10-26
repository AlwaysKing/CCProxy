// 项目模型状态层：候选缓存、按协议的绑定事务以及文件同步。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use super::{AppState, ProjectSyncStatusSnapshot};
use crate::clients::project_models::{
    apply_project_model, claude_settings_root, codex_project_trust, discover_default_projects,
    project_model_candidates, DiscoveredProject, ProjectModelCandidates, ProjectSources,
    ProjectSyncResult, ProjectSyncState,
};
use crate::config::{create_config_id, AppConfig, ProjectModelBinding};
use crate::logger;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedProjectRow {
    pub id: String,
    pub path: String,
    pub sources: ProjectSources,
    pub last_activity_ms: Option<i64>,
    pub missing: bool,
    pub codex_mapping_id: Option<String>,
    pub codex_model: Option<String>,
    pub claude_mapping_id: Option<String>,
    pub claude_model: Option<String>,
    pub codex_status: Option<ProjectSyncStatusSnapshot>,
    pub claude_status: Option<ProjectSyncStatusSnapshot>,
    pub codex_trusted: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedProjectModelsView {
    pub candidates: ProjectModelCandidates,
    pub projects: Vec<UnifiedProjectRow>,
    pub codex_daemon: Option<crate::clients::daemon::CodexDaemon>,
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn lexical_project_path(raw: &str) -> String {
    Path::new(raw.trim())
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .to_string()
}

fn project_identity(raw: &str) -> String {
    std::fs::canonicalize(raw)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| lexical_project_path(raw))
}

fn binding_mapping_model(config: &AppConfig, client: &str, mapping_id: &str) -> Option<String> {
    let protocol_id = if client == "codex" {
        "openai_responses"
    } else {
        "anthropic"
    };
    config
        .protocol(protocol_id)?
        .mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id && !mapping.client_model.contains('*'))
        .map(|mapping| mapping.client_model.clone())
}

fn sync_model_for_binding(config: &AppConfig, client: &str, mapping_id: &str) -> Option<String> {
    let protocol_id = if client == "codex" {
        "openai_responses"
    } else {
        "anthropic"
    };
    let protocol = config
        .protocol(protocol_id)
        .filter(|protocol| protocol.enabled)?;
    let mapping = protocol
        .mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)?;
    if !mapping.enabled || mapping.client_model.is_empty() || mapping.client_model.contains('*') {
        return None;
    }
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == mapping.provider_id && provider.enabled)?;
    provider
        .model_groups
        .iter()
        .any(|group| group.id == mapping.model_group_id)
        .then(|| mapping.client_model.clone())
}

fn reconcile_codex_trust(
    mut status: Option<ProjectSyncStatusSnapshot>,
    trusted: Option<bool>,
) -> Option<ProjectSyncStatusSnapshot> {
    if trusted == Some(true)
        && status
            .as_ref()
            .is_some_and(|item| item.status == ProjectSyncState::RequiresProjectTrust)
    {
        let item = status.as_mut().expect("状态刚检查过");
        item.status = ProjectSyncState::Ok;
        item.message = None;
    } else if trusted == Some(false)
        && status
            .as_ref()
            .is_some_and(|item| item.status == ProjectSyncState::Ok)
    {
        let item = status.as_mut().expect("状态刚检查过");
        item.status = ProjectSyncState::RequiresProjectTrust;
        item.message = Some("Codex 只会加载受信任项目的 config.toml".to_string());
    }
    status
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
            let Some(model) = sync_model_for_binding(config, &binding.client, &binding.mapping_id)
            else {
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

    pub async fn unified_project_models_view(
        self: &Arc<Self>,
        refresh: bool,
    ) -> UnifiedProjectModelsView {
        self.refresh_restart_required();
        let config = self.config_snapshot();
        let discovered = self.ensure_project_candidates(refresh).await;
        let statuses = self
            .project_sync_status
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let mut rows: BTreeMap<String, UnifiedProjectRow> = BTreeMap::new();

        for project in discovered {
            let id = project_identity(&project.path);
            rows.insert(
                id.clone(),
                UnifiedProjectRow {
                    id,
                    path: project.path,
                    sources: project.sources,
                    last_activity_ms: project.last_activity_ms,
                    missing: false,
                    codex_mapping_id: None,
                    codex_model: None,
                    claude_mapping_id: None,
                    claude_model: None,
                    codex_status: None,
                    claude_status: None,
                    codex_trusted: None,
                },
            );
        }

        for binding in &config.settings.project_model_bindings {
            let id = project_identity(&binding.path);
            let row = rows.entry(id.clone()).or_insert_with(|| UnifiedProjectRow {
                id,
                path: binding.path.clone(),
                sources: ProjectSources::default(),
                last_activity_ms: None,
                missing: !Path::new(&binding.path).is_dir(),
                codex_mapping_id: None,
                codex_model: None,
                claude_mapping_id: None,
                claude_model: None,
                codex_status: None,
                claude_status: None,
                codex_trusted: None,
            });
            let model = binding_mapping_model(&config, &binding.client, &binding.mapping_id);
            let status = statuses.get(&binding.id).cloned();
            if binding.client == "codex" {
                let trusted = self
                    .handle()
                    .is_some()
                    .then(|| codex_project_trust(&binding.path, &codex_home()))
                    .flatten();
                row.codex_mapping_id = Some(binding.mapping_id.clone());
                row.codex_model = model;
                row.codex_status = reconcile_codex_trust(status, trusted);
                row.codex_trusted = trusted;
            } else {
                row.claude_mapping_id = Some(binding.mapping_id.clone());
                row.claude_model = model;
                row.claude_status = status;
            }
        }

        let mut projects = rows.into_values().collect::<Vec<_>>();
        projects.sort_by(|left, right| left.path.cmp(&right.path));
        UnifiedProjectModelsView {
            candidates: project_model_candidates(&config),
            projects,
            codex_daemon: self.codex_daemon(),
        }
    }

    pub async fn set_project_model(
        self: &Arc<Self>,
        path: &str,
        client: &str,
        mapping_id: Option<&str>,
    ) -> Result<Option<ProjectSyncStatusSnapshot>, String> {
        if client != "codex" && client != "claude" {
            return Err("未知的客户端类型".to_string());
        }
        let path = path.trim();
        if path.is_empty() {
            return Err("项目目录不能为空".to_string());
        }
        let mapping_id = mapping_id.map(str::trim).filter(|value| !value.is_empty());
        let discovered = self.ensure_project_candidates(false).await;
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        let existing = draft
            .settings
            .project_model_bindings
            .iter()
            .position(|binding| binding.client == client && same_project_path(&binding.path, path));

        let Some(mapping_id) = mapping_id else {
            if let Some(index) = existing {
                draft.settings.project_model_bindings.remove(index);
                self.save_project_draft_locked(&draft)?;
            }
            return Ok(None);
        };

        let candidates = project_model_candidates(&draft);
        let candidate_list = if client == "codex" {
            &candidates.codex
        } else {
            &candidates.claude
        };
        let mapping = candidate_list
            .iter()
            .find(|candidate| candidate.mapping_id == mapping_id)
            .ok_or_else(|| "该映射当前不可作为项目默认模型".to_string())?;
        let discovered_path = discovered
            .iter()
            .find(|project| same_project_path(&project.path, path))
            .filter(|project| Path::new(&project.path).is_dir())
            .and_then(|project| std::fs::canonicalize(&project.path).ok())
            .map(|project| project.to_string_lossy().to_string());
        if existing.is_none() && discovered_path.is_none() {
            return Err("项目目录不存在或不在自动发现列表中".to_string());
        }
        let target = existing
            .map(|index| draft.settings.project_model_bindings[index].path.clone())
            .or(discovered_path)
            .ok_or_else(|| "项目目录不可用".to_string())?;

        if client == "claude" {
            let target_root = claude_root(&target);
            if let Some(owner) = draft
                .settings
                .project_model_bindings
                .iter()
                .enumerate()
                .find(|(index, binding)| {
                    Some(*index) != existing
                        && binding.client == "claude"
                        && claude_root(&binding.path) == target_root
                })
                .map(|(_, binding)| binding)
            {
                return Err(format!(
                    "项目 {target} 与 {} 使用同一个 Claude settings.local.json",
                    owner.path
                ));
            }
        }

        if let Some(index) = existing {
            draft.settings.project_model_bindings[index].mapping_id = mapping.mapping_id.clone();
        } else {
            draft
                .settings
                .project_model_bindings
                .push(ProjectModelBinding {
                    id: create_config_id("b"),
                    path: target,
                    client: client.to_string(),
                    mapping_id: mapping.mapping_id.clone(),
                });
        }
        self.save_project_draft_locked(&draft)?;
        Ok(draft
            .settings
            .project_model_bindings
            .iter()
            .find(|binding| binding.client == client && same_project_path(&binding.path, path))
            .and_then(|binding| {
                self.project_sync_status
                    .read()
                    .ok()
                    .and_then(|statuses| statuses.get(&binding.id).cloned())
            }))
    }

    fn save_project_draft_locked(self: &Arc<Self>, draft: &AppConfig) -> Result<AppConfig, String> {
        let value = serde_json::to_value(draft).map_err(|error| error.to_string())?;
        self.apply_config_locked(&value).map(|(config, _)| config)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::clients::project_models::project_model_candidates;
    use crate::config::validate_config;

    fn config_with_mappings(mappings: serde_json::Value) -> AppConfig {
        let result = validate_config(&json!({
            "version": 6,
            "providers": [{
                "id": "p1",
                "name": "provider",
                "enabled": true,
                "baseUrl": "https://example.com",
                "token": "sk-test",
                "modelGroups": [{
                    "id": "g1",
                    "apiFormat": "anthropic",
                    "models": [{ "name": "real" }]
                }]
            }],
            "protocols": [{
                "id": "anthropic",
                "name": "Anthropic",
                "enabled": true,
                "mappings": mappings
            }]
        }));
        assert!(result.ok, "{:?}", result.issues);
        result.config.expect("配置应有效")
    }

    fn status(state: ProjectSyncState) -> ProjectSyncStatusSnapshot {
        ProjectSyncStatusSnapshot {
            status: state,
            message: None,
            file: None,
            changed: false,
            synced_at_ms: 1,
            daemon_pid: None,
        }
    }

    #[test]
    fn shadowed_exact_mapping_stays_sync_valid_but_not_selectable() {
        let config = config_with_mappings(json!([
            {
                "id": "wild",
                "enabled": true,
                "clientModel": "claude-*",
                "providerId": "p1",
                "modelGroupId": "g1",
                "upstreamProtocol": "anthropic",
                "realModel": "real"
            },
            {
                "id": "exact",
                "enabled": true,
                "clientModel": "claude-a",
                "providerId": "p1",
                "modelGroupId": "g1",
                "upstreamProtocol": "anthropic",
                "realModel": "real"
            }
        ]));

        assert_eq!(
            sync_model_for_binding(&config, "claude", "exact"),
            Some("claude-a".to_string())
        );
        assert!(!project_model_candidates(&config)
            .claude
            .iter()
            .any(|candidate| candidate.mapping_id == "exact"));
    }

    #[test]
    fn disabled_bound_mapping_is_invalid_even_with_an_enabled_duplicate() {
        let config = config_with_mappings(json!([
            {
                "id": "disabled",
                "enabled": false,
                "clientModel": "claude-a",
                "providerId": "p1",
                "modelGroupId": "g1",
                "upstreamProtocol": "anthropic",
                "realModel": "real"
            },
            {
                "id": "enabled",
                "enabled": true,
                "clientModel": "claude-a",
                "providerId": "p1",
                "modelGroupId": "g1",
                "upstreamProtocol": "anthropic",
                "realModel": "real"
            }
        ]));

        assert_eq!(sync_model_for_binding(&config, "claude", "disabled"), None);
        assert!(project_model_candidates(&config)
            .claude
            .iter()
            .any(|candidate| candidate.mapping_id == "enabled"));
    }

    #[test]
    fn codex_trust_reconciles_both_directions() {
        let trusted = reconcile_codex_trust(
            Some(status(ProjectSyncState::RequiresProjectTrust)),
            Some(true),
        )
        .unwrap();
        assert_eq!(trusted.status, ProjectSyncState::Ok);

        let untrusted =
            reconcile_codex_trust(Some(status(ProjectSyncState::Ok)), Some(false)).unwrap();
        assert_eq!(untrusted.status, ProjectSyncState::RequiresProjectTrust);
        assert!(untrusted.message.is_some());
    }
}
