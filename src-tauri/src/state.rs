// 全局状态：配置、请求日志、代理生命周期、托盘与界面广播的统一出口。
// 与原 Electron 版一致：所有配置变更都从 apply_config 这一条路走，
// 落盘 → 更新内存 → 应用副作用 → 刷托盘 → 通知界面。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::clients::project_models::{ProjectListItem, ProjectModelCandidates, ProjectSyncState};
use crate::clients::{apply_client_config, expected_catalog_entries, scan_client_dirs};
use crate::config::{create_client_token, AppConfig, ConfigManager, DEFAULT_PORT};
use crate::logger;
use crate::proxy::Proxy;
use crate::request_log::RequestLog;
use crate::tray;

mod project_models;

#[derive(Serialize, Clone, Debug, Default)]
pub struct ProxyStatus {
    pub status: String,
    pub port: Option<u16>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSyncStatusSnapshot {
    pub status: ProjectSyncState,
    pub message: Option<String>,
    pub file: Option<String>,
    pub changed: bool,
    pub synced_at_ms: i64,
    pub daemon_pid: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelRow {
    #[serde(flatten)]
    pub project: ProjectListItem,
    pub codex_status: Option<ProjectSyncStatusSnapshot>,
    pub claude_status: Option<ProjectSyncStatusSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelsView {
    pub enabled: bool,
    pub candidates: ProjectModelCandidates,
    pub projects: Vec<ProjectModelRow>,
    pub codex_daemon: Option<crate::clients::daemon::CodexDaemon>,
}

pub struct AppState {
    handle: OnceLock<AppHandle>,
    pub config_manager: ConfigManager,
    pub request_log: RequestLog,
    pub proxy: Proxy,
    // 请求体上限：默认 64MB，测试里可以调小来验 413
    max_body_bytes: AtomicUsize,
    pub config: RwLock<AppConfig>,
    pub issues: RwLock<Vec<String>>,
    pub recovered: RwLock<bool>,
    pub backup_path: RwLock<Option<String>>,
    pub proxy_status: RwLock<ProxyStatus>,
    config_transaction: Mutex<()>,
    project_sync_status: RwLock<HashMap<(String, String), ProjectSyncStatusSnapshot>>,
    pub http_client: reqwest::Client,
}

impl AppState {
    pub fn new(config_manager: ConfigManager, request_log: RequestLog, config: AppConfig) -> Self {
        // 上游连接：不跟随重定向（与 Node 的 http.request 一致），超时由转发层逐档控制
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            handle: OnceLock::new(),
            config_manager,
            request_log,
            proxy: Proxy::new(),
            max_body_bytes: AtomicUsize::new(crate::proxy::server::MAX_BODY_BYTES),
            config: RwLock::new(config),
            issues: RwLock::new(Vec::new()),
            recovered: RwLock::new(false),
            backup_path: RwLock::new(None),
            proxy_status: RwLock::new(ProxyStatus {
                status: "stopped".to_string(),
                port: None,
                error: None,
            }),
            config_transaction: Mutex::new(()),
            project_sync_status: RwLock::new(HashMap::new()),
            http_client,
        }
    }

    pub fn set_handle(&self, handle: AppHandle) {
        let _ = self.handle.set(handle);
    }

    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes.load(Ordering::Relaxed)
    }

    pub fn set_max_body_bytes(&self, value: usize) {
        self.max_body_bytes.store(value, Ordering::Relaxed);
    }

    pub fn handle(&self) -> Option<&AppHandle> {
        self.handle.get()
    }

    pub fn emit(&self, event: &str, payload: &Value) {
        if let Some(handle) = self.handle.get() {
            let _ = handle.emit(event, payload.clone());
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.config
            .read()
            .map(|config| config.clone())
            .unwrap_or_else(|_| crate::config::create_default_config())
    }

    pub fn port(&self) -> u16 {
        self.config
            .read()
            .map(|config| config.settings.port)
            .unwrap_or(DEFAULT_PORT)
    }

    pub fn proxy_status(&self) -> ProxyStatus {
        self.proxy_status
            .read()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn set_proxy_status(&self, next: ProxyStatus) {
        if let Ok(mut slot) = self.proxy_status.write() {
            *slot = next.clone();
        }
        tray::refresh(self);
        self.emit(
            "proxy-status-changed",
            &serde_json::to_value(&next).unwrap_or(Value::Null),
        );
    }

    pub async fn start_proxy(self: &Arc<Self>) -> ProxyStatus {
        let state = self.clone();
        let port = state.port();
        let next = match state.proxy.start().await {
            Ok(bound) => ProxyStatus {
                status: "running".to_string(),
                port: Some(bound),
                error: None,
            },
            Err(error) => ProxyStatus {
                status: "error".to_string(),
                port: Some(port),
                error: Some(error),
            },
        };
        state.set_proxy_status(next.clone());
        logger::info(
            "[Proxy] 启动结果",
            Some(&json!({ "status": next.status, "port": next.port, "error": next.error })),
        );
        next
    }

    pub async fn stop_proxy(self: &Arc<Self>) -> ProxyStatus {
        self.proxy.stop().await;
        let next = ProxyStatus {
            status: "stopped".to_string(),
            port: Some(self.port()),
            error: None,
        };
        self.set_proxy_status(next.clone());
        logger::info("[Proxy] 已停止", None);
        next
    }

    pub async fn toggle_proxy(self: &Arc<Self>) -> ProxyStatus {
        if self.proxy_status().status == "running" {
            self.stop_proxy().await
        } else {
            self.start_proxy().await
        }
    }

    pub async fn restart_proxy(self: &Arc<Self>) -> ProxyStatus {
        self.stop_proxy().await;
        self.start_proxy().await
    }

    // codex 模型清单的期望条目：OpenAI 协议下启用的映射 → 名字用 clientModel、
    // 能力取映射指向的真实模型。检查与应用共用这一份，两边口径才不会漂
    pub fn codex_catalog(&self, config: &AppConfig) -> Vec<crate::clients::catalog::ExpectedEntry> {
        // codex 只会发 Responses 协议，所以清单取自 openai_responses 这条（协议拆分前是 openai）
        expected_catalog_entries(config.protocol("openai_responses"), &config.providers)
    }

    // 协议配置变了（模型能力、映射、供应商启停）就把已归我们管的 codex 目录的清单刷一遍，
    // 用户改完配置不用再跑去设置页点「应用」。没归我们管的目录不动，留给手动应用。
    // 失败只记日志不打断：清单没刷成，下一次目录检查会把不一致报出来
    pub fn sync_managed_codex_catalogs(self: &Arc<Self>) {
        let config = self.config_snapshot();
        let scan = scan_client_dirs(None, &config.settings.client_dirs);
        let catalog = self.codex_catalog(&config);
        let mut entries = scan.builtin;
        entries.extend(scan.manual);
        let result = crate::clients::sync_codex_catalogs(
            &entries,
            config.settings.port,
            &config.settings.client_token,
            &catalog,
        );
        if !result.applied.is_empty() {
            // 顺手带上 daemon 死活：磁盘改完而 daemon 还在跑，用户看到的就是「改了没生效」，
            // 排查时这行日志能省一次困惑
            let daemon_running = entries.iter().any(|entry| {
                entry.kind == "codex" && crate::clients::daemon::inspect(&entry.path).running
            });
            logger::info(
                "[ClientDirs] 模型清单已随配置自动更新",
                Some(&json!({
                    "files": result.applied.iter().map(|item| item.file.clone()).collect::<Vec<String>>(),
                    "codexDaemonRunning": daemon_running
                })),
            );
        }
        for (path, message) in result.errors {
            logger::error(
                "[ClientDirs] 模型清单自动更新失败",
                Some(&json!({ "path": path, "error": message })),
            );
        }
    }

    // 配置变更的副作用：端口改了要按新端口重启、开机自启要同步给系统。
    // 放在这里而不是渲染层——托盘、设置页或以后别处改配置都走同一条路
    pub fn apply_config_effects(self: &Arc<Self>, previous: Option<&AppConfig>, next: &AppConfig) {
        if previous.map(|config| config.settings.launch_at_login)
            != Some(next.settings.launch_at_login)
        {
            if let Some(handle) = self.handle.get() {
                use tauri_plugin_autostart::ManagerExt;
                let manager = handle.autolaunch();
                let result = if next.settings.launch_at_login {
                    manager.enable()
                } else {
                    manager.disable()
                };
                if let Err(error) = result {
                    logger::warn(
                        "[App] 同步开机自启失败",
                        Some(&json!({ "error": error.to_string() })),
                    );
                }
            }
        }

        let port_changed = previous.map(|config| config.settings.port) != Some(next.settings.port);
        let running = self
            .proxy_status
            .read()
            .map(|status| status.status == "running")
            .unwrap_or(false);
        // 只有正在跑才需要重启；停着的话下次启动自然读新端口
        if port_changed && running {
            logger::info(
                "[Proxy] 端口变更，按新端口重启",
                Some(&json!({ "port": next.settings.port })),
            );
            let state = self.clone();
            tauri::async_runtime::spawn(async move {
                let status = state.restart_proxy().await;
                if status.status != "running" {
                    logger::error(
                        "[Proxy] 端口变更后重启失败",
                        Some(&json!({ "error": status.error })),
                    );
                }
            });
        }

        // 协议配置一变，已托管 codex 目录的模型清单就过期了——趁广播通知界面之前刷掉，
        // 界面随后重扫时看到的就是新结果，不会闪一个「不一致」出来
        let protocols_changed = previous
            .map(|config| config.protocols != next.protocols)
            .unwrap_or(true);
        let providers_changed = previous
            .map(|config| config.providers != next.providers)
            .unwrap_or(true);
        let projects_changed = previous
            .map(|config| {
                config.settings.project_model_sync_enabled
                    != next.settings.project_model_sync_enabled
                    || config.settings.project_model_projects
                        != next.settings.project_model_projects
            })
            .unwrap_or(true);
        if protocols_changed {
            self.sync_managed_codex_catalogs();
        }
        if next.settings.project_model_sync_enabled
            && (projects_changed || protocols_changed || providers_changed)
        {
            self.sync_all_project_models_locked(next);
        }
    }

    // 所有配置变更的唯一出口。事务锁串行化完整配置的读取、保存和副作用。
    pub fn apply_config(
        self: &Arc<Self>,
        next: &Value,
    ) -> Result<(AppConfig, Vec<String>), String> {
        let _guard = self
            .config_transaction
            .lock()
            .map_err(|_| "配置事务锁不可用".to_string())?;
        self.apply_config_locked(next)
    }

    pub(super) fn apply_config_locked(
        self: &Arc<Self>,
        next: &Value,
    ) -> Result<(AppConfig, Vec<String>), String> {
        let previous = self.config.read().ok().map(|config| config.clone());
        let (config, issues) = self.config_manager.save(next)?;
        if let Ok(mut slot) = self.config.write() {
            *slot = config.clone();
        }
        if let Ok(mut slot) = self.issues.write() {
            *slot = issues.clone();
        }
        // 用户主动存过配置后，磁盘上已经是这份新配置，不再有「重建出来的那份」，
        // 重建横幅必须退场，否则后续的校验问题会被错并进「已重建」里
        if let Ok(mut slot) = self.recovered.write() {
            *slot = false;
        }
        if let Ok(mut slot) = self.backup_path.write() {
            *slot = None;
        }
        self.apply_config_effects(previous.as_ref(), &config);
        tray::refresh(self);
        self.emit(
            "config-changed",
            &json!({
                "config": config,
                "issues": issues,
                "recovered": false,
                "backupPath": ""
            }),
        );
        Ok((config, issues))
    }

    // 手动目录的增删、扫描剔除、kind 矫正都改的是配置，统一从这条口落盘
    pub fn persist_client_dirs(
        self: &Arc<Self>,
        dirs: Vec<crate::config::ClientDir>,
    ) -> Result<(), String> {
        let mut draft = self
            .config
            .read()
            .map_err(|_| "配置不可读".to_string())?
            .clone();
        draft.settings.client_dirs = dirs;
        let value = serde_json::to_value(&draft).map_err(|error| error.to_string())?;
        self.apply_config(&value)?;
        Ok(())
    }

    // 重新生成客户端 token：与 persist_client_dirs 一样走 apply_config 落盘并广播。
    // 原本就配好（地址与 token 都对）的目录属于「被换 token 连坐」，自动跟着写入新值；
    // 原本就不一致的本来就要人工处理，不趁机动它——所以 ok 名单必须按旧 token 先扫出来
    pub fn regenerate_client_token(
        self: &Arc<Self>,
    ) -> Result<(AppConfig, Vec<String>, Vec<Value>), String> {
        let before = self.config_snapshot();
        let scan = scan_client_dirs(None, &before.settings.client_dirs);
        let mut entries = scan.builtin;
        entries.extend(scan.manual);
        let was_ok: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let context = crate::clients::check::InspectContext::new(
                    before.settings.port,
                    &before.settings.client_token,
                    Vec::new(),
                );
                crate::clients::inspect_client_config(&entry.path, &entry.kind, &context).status
                    == "ok"
            })
            .collect();

        let mut draft = before.clone();
        draft.settings.client_token = create_client_token();
        let value = serde_json::to_value(&draft).map_err(|error| error.to_string())?;
        let (config, _) = self.apply_config(&value)?;
        logger::info("[Config] 已重新生成客户端 token", None);

        let catalog = self.codex_catalog(&config);
        let mut reapplied: Vec<String> = Vec::new();
        let mut errors: Vec<Value> = Vec::new();
        for entry in was_ok {
            match apply_client_config(
                &entry.path,
                &entry.kind,
                config.settings.port,
                &config.settings.client_token,
                &catalog,
            ) {
                Ok(results) => {
                    reapplied.push(entry.path.clone());
                    logger::info(
                        "[ClientDirs] 换 token 后已自动写入",
                        Some(&json!({
                            "kind": entry.kind,
                            "path": entry.path,
                            "files": results.iter().map(|item| item.file.clone()).collect::<Vec<String>>()
                        })),
                    );
                }
                Err(message) => {
                    errors.push(json!({ "path": entry.path, "message": message }));
                    logger::error(
                        "[ClientDirs] 换 token 后自动写入失败",
                        Some(&json!({ "path": entry.path, "error": message })),
                    );
                }
            }
        }
        Ok((config, reapplied, errors))
    }
}
