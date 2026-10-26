// 项目模型状态层：配置事务、运行时同步状态和项目级 Tauri 操作。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::{AppState, ProjectModelRow, ProjectModelsView, ProjectSyncStatusSnapshot};
use crate::clients::project_models::{
    apply_project_model, claude_settings_root, discover_default_projects, merge_project_list,
    project_model_candidates, ProjectSyncResult, ProjectSyncState,
};
use crate::config::{create_config_id, AppConfig, ProjectModelProject};
use crate::logger;

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
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

impl AppState {
    fn codex_daemon(&self) -> Option<crate::clients::daemon::CodexDaemon> {
        self.handle()?;
        Some(crate::clients::daemon::inspect(
            &codex_home().to_string_lossy(),
        ))
    }

    fn record_project_sync(
        &self,
        project_id: &str,
        client: &str,
        result: ProjectSyncResult,
        daemon_pid: Option<u32>,
    ) {
        let mut status = result.status.clone();
        let mut message = result.message.clone();
        if status == ProjectSyncState::Ok && client == "codex" && daemon_pid.is_some() {
            status = ProjectSyncState::RestartRequired;
            message =
                Some("Codex app-server daemon 正在运行，重启后才会读取新的项目模型".to_string());
        }
        let snapshot = ProjectSyncStatusSnapshot {
            status,
            message,
            file: result.file,
            changed: result.changed,
            synced_at_ms: crate::now_ms(),
            daemon_pid,
        };
        if let Ok(mut statuses) = self.project_sync_status.write() {
            statuses.insert((project_id.to_string(), client.to_string()), snapshot);
        }
    }

    pub(super) fn sync_all_project_models_locked(self: &Arc<Self>, config: &AppConfig) {
        if !config.settings.project_model_sync_enabled {
            return;
        }
        let daemon_pid = self
            .codex_daemon()
            .filter(|daemon| daemon.running)
            .and_then(|daemon| daemon.pid);
        let mut claude_roots: HashMap<PathBuf, String> = HashMap::new();
        for project in &config.settings.project_model_projects {
            if let Some(model) = project
                .codex_model
                .as_deref()
                .filter(|model| !model.is_empty())
            {
                let result = apply_project_model(config, &project.path, "codex", model);
                if result.status != ProjectSyncState::Ok {
                    logger::warn(
                        "[ProjectModels] Codex 同步未完成",
                        Some(&json!({
                            "project": project.path,
                            "status": result.status,
                            "message": result.message
                        })),
                    );
                }
                self.record_project_sync(&project.id, "codex", result, daemon_pid);
            }
            if let Some(model) = project
                .claude_model
                .as_deref()
                .filter(|model| !model.is_empty())
            {
                let root = std::fs::canonicalize(claude_settings_root(&project.path))
                    .unwrap_or_else(|_| claude_settings_root(&project.path));
                if let Some(owner) = claude_roots.get(&root) {
                    self.record_project_sync(
                        &project.id,
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
                    );
                    continue;
                }
                claude_roots.insert(root, project.path.clone());
                let result = apply_project_model(config, &project.path, "claude", model);
                if result.status != ProjectSyncState::Ok {
                    logger::warn(
                        "[ProjectModels] Claude 同步未完成",
                        Some(&json!({
                            "project": project.path,
                            "status": result.status,
                            "message": result.message
                        })),
                    );
                }
                self.record_project_sync(&project.id, "claude", result, None);
            }
        }

        if let Ok(mut statuses) = self.project_sync_status.write() {
            statuses.retain(|(project_id, client), _| {
                config
                    .settings
                    .project_model_projects
                    .iter()
                    .find(|project| project.id == *project_id)
                    .is_some_and(|project| match client.as_str() {
                        "codex" => project.codex_model.is_some(),
                        "claude" => project.claude_model.is_some(),
                        _ => false,
                    })
            });
        }
        self.emit(
            "project-models-changed",
            &json!({
                "enabled": config.settings.project_model_sync_enabled,
                "syncedAt": crate::now_ms()
            }),
        );
    }

    fn refresh_restart_required(&self) {
        let Some(daemon) = self.codex_daemon() else {
            return;
        };
        if let Ok(mut statuses) = self.project_sync_status.write() {
            for ((_, client), status) in statuses.iter_mut() {
                if client != "codex" || status.status != ProjectSyncState::RestartRequired {
                    continue;
                }
                if !daemon.running || daemon.pid != status.daemon_pid {
                    status.status = ProjectSyncState::Ok;
                    status.message = None;
                    status.daemon_pid = None;
                }
            }
        }
    }

    pub fn project_models_view(self: &Arc<Self>) -> ProjectModelsView {
        self.refresh_restart_required();
        let config = self.config_snapshot();
        let discovered = if config.settings.project_model_sync_enabled {
            discover_default_projects()
        } else {
            Vec::new()
        };
        let projects = merge_project_list(&config, discovered);
        let statuses = self
            .project_sync_status
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let projects = projects
            .into_iter()
            .map(|project| ProjectModelRow {
                codex_status: statuses
                    .get(&(project.id.clone(), "codex".to_string()))
                    .cloned(),
                claude_status: statuses
                    .get(&(project.id.clone(), "claude".to_string()))
                    .cloned(),
                project,
            })
            .collect();
        ProjectModelsView {
            enabled: config.settings.project_model_sync_enabled,
            candidates: project_model_candidates(&config),
            projects,
            codex_daemon: if config.settings.project_model_sync_enabled {
                self.codex_daemon()
            } else {
                None
            },
        }
    }

    fn save_project_draft_locked(self: &Arc<Self>, draft: &AppConfig) -> Result<AppConfig, String> {
        let value = serde_json::to_value(draft).map_err(|error| error.to_string())?;
        self.apply_config_locked(&value).map(|(config, _)| config)
    }

    pub fn set_project_sync_enabled(self: &Arc<Self>, enabled: bool) -> Result<AppConfig, String> {
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        draft.settings.project_model_sync_enabled = enabled;
        self.save_project_draft_locked(&draft)
    }

    pub fn add_project_dir(self: &Arc<Self>, path: &str) -> Result<(), String> {
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let target =
            std::fs::canonicalize(path).map_err(|error| format!("项目目录不可用：{error}"))?;
        if !target.is_dir() {
            return Err("选择的路径不是目录".to_string());
        }
        let target = target.to_string_lossy().to_string();
        let mut draft = self.config_snapshot();
        if draft
            .settings
            .project_model_projects
            .iter()
            .any(|project| same_project_path(&project.path, &target))
        {
            return Ok(());
        }
        let known = discover_default_projects()
            .iter()
            .any(|project| same_project_path(&project.path, &target));
        draft
            .settings
            .project_model_projects
            .push(ProjectModelProject {
                id: create_config_id("p"),
                path: target,
                codex_model: None,
                claude_model: None,
                manual: !known,
                hidden: false,
            });
        self.save_project_draft_locked(&draft)?;
        Ok(())
    }

    pub fn set_project_model(
        self: &Arc<Self>,
        path: &str,
        client: &str,
        model: &str,
    ) -> Result<ProjectSyncStatusSnapshot, String> {
        if client != "codex" && client != "claude" {
            return Err("未知的客户端类型".to_string());
        }
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let target = std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .to_string_lossy()
            .to_string();
        let mut draft = self.config_snapshot();
        let index = draft
            .settings
            .project_model_projects
            .iter()
            .position(|project| same_project_path(&project.path, &target));
        let index = match index {
            Some(index) => index,
            None => {
                if !discover_default_projects()
                    .iter()
                    .any(|project| same_project_path(&project.path, &target))
                {
                    return Err("项目目录不在自动发现或手动添加列表中".to_string());
                }
                draft
                    .settings
                    .project_model_projects
                    .push(ProjectModelProject {
                        id: create_config_id("p"),
                        path: target,
                        codex_model: None,
                        claude_model: None,
                        manual: false,
                        hidden: false,
                    });
                draft.settings.project_model_projects.len() - 1
            }
        };
        let value = model.trim();
        let value = (!value.is_empty()).then(|| value.to_string());
        if client == "claude" && value.is_some() {
            let target_root =
                claude_settings_root(&draft.settings.project_model_projects[index].path);
            let target_root = std::fs::canonicalize(&target_root).unwrap_or(target_root);
            if let Some(owner) = draft
                .settings
                .project_model_projects
                .iter()
                .find(|project| {
                    project.id != draft.settings.project_model_projects[index].id
                        && project.claude_model.is_some()
                        && {
                            let root = claude_settings_root(&project.path);
                            std::fs::canonicalize(&root).unwrap_or(root) == target_root
                        }
                })
            {
                return Err(format!(
                    "项目 {} 与 {} 使用同一个 Claude settings.local.json，请先清除另一个项目的 Claude 绑定",
                    draft.settings.project_model_projects[index].path, owner.path
                ));
            }
        }
        if client == "codex" {
            draft.settings.project_model_projects[index].codex_model = value;
        } else {
            draft.settings.project_model_projects[index].claude_model = value;
        }
        let project_id = draft.settings.project_model_projects[index].id.clone();
        let config = self.save_project_draft_locked(&draft)?;
        // 改变绑定会走配置副作用同步；只有内容完全相同、没有触发副作用时才补一次。
        let has_status = self
            .project_sync_status
            .read()
            .is_ok_and(|statuses| statuses.contains_key(&(project_id.clone(), client.to_string())));
        if !has_status {
            self.sync_all_project_models_locked(&config);
        }
        if let Some(status) = self
            .project_sync_status
            .read()
            .ok()
            .and_then(|statuses| statuses.get(&(project_id, client.to_string())).cloned())
        {
            return Ok(status);
        }
        Ok(ProjectSyncStatusSnapshot {
            status: if config.settings.project_model_sync_enabled {
                ProjectSyncState::Ok
            } else {
                ProjectSyncState::SyncDisabled
            },
            message: (!config.settings.project_model_sync_enabled)
                .then(|| "项目模型同步已关闭".to_string()),
            file: None,
            changed: false,
            synced_at_ms: crate::now_ms(),
            daemon_pid: None,
        })
    }

    pub fn set_project_hidden(self: &Arc<Self>, path: &str, hidden: bool) -> Result<(), String> {
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        if let Some(project) = draft
            .settings
            .project_model_projects
            .iter_mut()
            .find(|project| same_project_path(&project.path, path))
        {
            project.hidden = hidden;
        } else {
            let target =
                std::fs::canonicalize(path).map_err(|error| format!("项目目录不可用：{error}"))?;
            if !discover_default_projects()
                .iter()
                .any(|project| same_project_path(&project.path, &target.to_string_lossy()))
            {
                return Err("项目目录不在自动发现或手动添加列表中".to_string());
            }
            draft
                .settings
                .project_model_projects
                .push(ProjectModelProject {
                    id: create_config_id("p"),
                    path: target.to_string_lossy().to_string(),
                    codex_model: None,
                    claude_model: None,
                    manual: false,
                    hidden,
                });
        }
        self.save_project_draft_locked(&draft)?;
        Ok(())
    }

    pub fn remove_project(self: &Arc<Self>, path: &str) -> Result<(), String> {
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        let mut draft = self.config_snapshot();
        let project = draft
            .settings
            .project_model_projects
            .iter()
            .find(|project| same_project_path(&project.path, path))
            .cloned();
        if let Some(project) = project {
            if project.manual || project.hidden {
                draft
                    .settings
                    .project_model_projects
                    .retain(|item| item.id != project.id);
            } else {
                for item in &mut draft.settings.project_model_projects {
                    if item.id == project.id {
                        item.codex_model = None;
                        item.claude_model = None;
                    }
                }
                draft.settings.project_model_projects.retain(|item| {
                    item.codex_model.is_some()
                        || item.claude_model.is_some()
                        || item.manual
                        || item.hidden
                });
            }
        }
        self.save_project_draft_locked(&draft)?;
        Ok(())
    }
}
