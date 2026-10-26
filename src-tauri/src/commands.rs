// 前端命令：与原 Electron 版的 ipcMain handler 一一对应，返回结构也保持一致
// （统一 { success, ... }，失败时带 error 文案）。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::clients::{
    apply_client_config, daemon, detect_dir_kind, inspect_client_config, scan_client_dirs,
};
use crate::config::{create_config_id, ClientDir};
use crate::logger;
use crate::proxy::fetch_models::fetch_upstream_models;
use crate::state::AppState;

fn error_value(error: impl ToString) -> Value {
    json!({ "success": false, "error": error.to_string() })
}

#[tauri::command]
pub async fn get_app_state(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let config = state.config_snapshot();
    let issues = state
        .issues
        .read()
        .map(|value| value.clone())
        .unwrap_or_default();
    let recovered = state.recovered.read().map(|value| *value).unwrap_or(false);
    let backup_path = state
        .backup_path
        .read()
        .ok()
        .and_then(|value| value.clone());
    let proxy = state.proxy_status();
    Ok(json!({
        "success": true,
        "config": config,
        "issues": issues,
        "recovered": recovered,
        "backupPath": backup_path,
        "configPath": state.config_manager.path(),
        "proxy": proxy
    }))
}

#[tauri::command]
pub async fn get_config(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    Ok(json!({ "success": true, "config": state.config_snapshot() }))
}

#[tauri::command]
pub async fn save_config(state: State<'_, Arc<AppState>>, config: Value) -> Result<Value, String> {
    match state.apply_config(&config) {
        Ok((saved, issues)) => Ok(json!({ "success": true, "config": saved, "issues": issues })),
        Err(error) => {
            logger::error("[Config] 保存失败", Some(&json!({ "error": error })));
            Ok(error_value(error))
        }
    }
}

#[tauri::command]
pub async fn regenerate_client_token(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    match state.regenerate_client_token() {
        Ok((config, reapplied, errors)) => Ok(
            json!({ "success": true, "config": config, "reapplied": reapplied, "errors": errors }),
        ),
        Err(error) => {
            logger::error(
                "[Config] 重新生成客户端 token 失败",
                Some(&json!({ "error": error })),
            );
            Ok(error_value(error))
        }
    }
}

#[tauri::command]
pub async fn get_client_dirs(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let config = state.config_snapshot();
    let manual = config.settings.client_dirs.clone();
    let scan = scan_client_dirs(None, &manual);
    // 剔除与矫正都要落盘；扫描保持原顺序，整体序列化比对就够用
    if serde_json::to_value(&scan.manual).ok() != serde_json::to_value(&manual).ok() {
        if let Err(error) = state.persist_client_dirs(scan.manual.clone()) {
            return Ok(error_value(error));
        }
    }

    // 检查结果跟着目录一起返回：界面拿到的每条目录自带「有没有指向 CCProxy」的状态。
    // 期望地址由当前端口与客户端 token 推出，所以端口或 token 一变，下一次取列表就是新结果
    let catalog = state.codex_catalog(&config);
    let context = crate::clients::check::InspectContext::new(
        config.settings.port,
        &config.settings.client_token,
        catalog,
    );
    let decorate = |entry: &ClientDir| -> Value {
        let inspected = inspect_client_config(&entry.path, &entry.kind, &context);
        let mut value = serde_json::to_value(entry).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("status".to_string(), Value::String(inspected.status));
            object.insert("target".to_string(), Value::String(inspected.target));
            object.insert("problems".to_string(), json!(inspected.problems));
        }
        value
    };

    Ok(json!({
        "success": true,
        "builtin": scan.builtin.iter().map(decorate).collect::<Vec<Value>>(),
        "manual": scan.manual.iter().map(decorate).collect::<Vec<Value>>(),
        "dropped": scan.dropped,
        "expectedAddress": format!("http://127.0.0.1:{}", config.settings.port)
    }))
}

// codex 的 app-server daemon 体检。它只在启动时读一次 config.toml 与模型清单，
// 磁盘改完它还是旧的——设置页据此提醒用户执行 `codex app-server daemon restart`。
// 有多个 codex 目录时优先报正在跑的那个；一个都没跑就报第一个，界面上换成条件式说法
#[tauri::command]
pub async fn get_codex_daemon(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let config = state.config_snapshot();
    let scan = scan_client_dirs(None, &config.settings.client_dirs);
    let mut entries = scan.builtin;
    entries.extend(scan.manual);

    let mut first: Option<daemon::CodexDaemon> = None;
    for entry in entries.iter().filter(|item| item.kind == "codex") {
        let status = daemon::inspect(&entry.path);
        if status.running {
            return Ok(json!({ "success": true, "daemon": status }));
        }
        first = first.or(Some(status));
    }
    Ok(json!({ "success": true, "daemon": first }))
}

#[tauri::command]
pub async fn pick_client_dir(app: tauri::AppHandle) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择客户端配置目录")
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    let picked = receiver.await.ok().flatten();
    Ok(json!({
        "success": true,
        "path": picked.map(|path| path.to_string())
    }))
}

#[tauri::command]
pub async fn add_client_dir(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Value, String> {
    let target = path.trim();
    if target.is_empty() {
        return Ok(error_value("没有选择目录"));
    }
    let normalized = PathBuf::from(target);
    let Some(kind) = detect_dir_kind(target) else {
        return Ok(error_value("不是 Claude 或 Codex 的配置目录"));
    };
    // 内置位置是常量，有效时会自己出现在列表里，手动存一份只会重复
    if crate::clients::dirs::builtin_dirs(None)
        .iter()
        .any(|entry| PathBuf::from(&entry.path) == normalized)
    {
        return Ok(error_value("这是内置扫描位置，不用手动添加"));
    }
    let config = state.config_snapshot();
    if config
        .settings
        .client_dirs
        .iter()
        .any(|entry| PathBuf::from(&entry.path) == normalized)
    {
        return Ok(error_value("该目录已在列表中"));
    }

    let mut dirs = config.settings.client_dirs.clone();
    dirs.push(ClientDir {
        id: create_config_id("d"),
        kind: kind.clone(),
        path: normalized.to_string_lossy().to_string(),
    });
    if let Err(error) = state.persist_client_dirs(dirs) {
        return Ok(error_value(error));
    }
    logger::info(
        "[ClientDirs] 已添加手动目录",
        Some(&json!({ "kind": kind, "path": target })),
    );
    Ok(json!({ "success": true, "kind": kind }))
}

// 把 CCProxy 的地址与 token 写进该目录的配置文件。只认扫描出来的目录：
// 渲染层传什么路径就写什么的话，这条口就成了「往任意文件写内容」
#[tauri::command]
pub async fn apply_client_dir(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Value, String> {
    let target = path.trim();
    let config = state.config_snapshot();
    let scan = scan_client_dirs(None, &config.settings.client_dirs);
    let mut entries = scan.builtin;
    entries.extend(scan.manual);
    let Some(entry) = entries.into_iter().find(|entry| entry.path == target) else {
        return Ok(error_value("该目录不在扫描结果里，请先刷新"));
    };
    let catalog = state.codex_catalog(&config);
    match apply_client_config(
        &entry.path,
        &entry.kind,
        config.settings.port,
        &config.settings.client_token,
        &catalog,
    ) {
        Ok(results) => {
            logger::info(
                "[ClientDirs] 已写入 CCProxy 配置",
                Some(&json!({
                    "kind": entry.kind,
                    "files": results.iter().map(|item| item.file.clone()).collect::<Vec<String>>()
                })),
            );
            Ok(json!({ "success": true, "files": results }))
        }
        Err(error) => {
            logger::error("[ClientDirs] 写入失败", Some(&json!({ "error": error })));
            Ok(error_value(error))
        }
    }
}

#[tauri::command]
pub async fn remove_client_dir(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Value, String> {
    let config = state.config_snapshot();
    let dirs: Vec<ClientDir> = config
        .settings
        .client_dirs
        .into_iter()
        .filter(|entry| entry.id != id)
        .collect();
    match state.persist_client_dirs(dirs) {
        Ok(()) => Ok(json!({ "success": true })),
        Err(error) => Ok(error_value(error)),
    }
}

#[tauri::command]
pub async fn get_project_models(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let view = state.unified_project_models_view(false).await;
    Ok(json!({
        "success": true,
        "view": serde_json::to_value(view).unwrap_or(Value::Null)
    }))
}

#[tauri::command]
pub async fn refresh_project_models(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let view = state.unified_project_models_view(true).await;
    Ok(json!({
        "success": true,
        "view": serde_json::to_value(view).unwrap_or(Value::Null)
    }))
}

#[tauri::command]
pub async fn set_project_model(
    state: State<'_, Arc<AppState>>,
    path: String,
    client: String,
    mapping_id: Option<String>,
) -> Result<Value, String> {
    match state
        .set_project_model(path.trim(), &client, mapping_id.as_deref())
        .await
    {
        Ok(status) => Ok(json!({ "success": true, "status": status })),
        Err(error) => Ok(error_value(error)),
    }
}

#[tauri::command]
pub async fn get_proxy_status(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let status = state.proxy_status();
    Ok(
        json!({ "success": true, "status": status.status, "port": status.port, "error": status.error }),
    )
}

#[tauri::command]
pub async fn start_proxy(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let inner = state.inner().clone();
    let status = inner.start_proxy().await;
    Ok(json!({
        "success": status.status == "running",
        "status": status.status,
        "port": status.port,
        "error": status.error
    }))
}

#[tauri::command]
pub async fn stop_proxy(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let inner = state.inner().clone();
    let status = inner.stop_proxy().await;
    Ok(
        json!({ "success": true, "status": status.status, "port": status.port, "error": status.error }),
    )
}

#[tauri::command]
pub async fn restart_proxy(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let inner = state.inner().clone();
    let status = inner.restart_proxy().await;
    Ok(json!({
        "success": status.status == "running",
        "status": status.status,
        "port": status.port,
        "error": status.error
    }))
}

#[tauri::command]
pub async fn fetch_provider_models(
    state: State<'_, Arc<AppState>>,
    payload: Value,
) -> Result<Value, String> {
    let client = state.http_client.clone();
    Ok(fetch_upstream_models(&client, &payload).await)
}

#[tauri::command]
pub async fn get_logs(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    Ok(json!({ "success": true, "logs": state.request_log.list() }))
}

#[tauri::command]
pub async fn get_usage(
    state: State<'_, Arc<AppState>>,
    range: Option<Value>,
) -> Result<Value, String> {
    let options = range.unwrap_or_else(|| json!({}));
    let usage = state.request_log.usage(&options);
    let mut value = json!({ "success": true });
    if let (Some(target), Some(source)) = (value.as_object_mut(), usage.as_object()) {
        for (key, item) in source {
            target.insert(key.clone(), item.clone());
        }
    }
    Ok(value)
}

#[tauri::command]
pub async fn get_storage_stats(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let stats = state.request_log.stats();
    let mut value = json!({ "success": true });
    if let (Some(target), Some(source)) = (value.as_object_mut(), stats.as_object()) {
        for (key, item) in source {
            target.insert(key.clone(), item.clone());
        }
    }
    Ok(value)
}

#[tauri::command]
pub async fn clear_logs(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    state.request_log.clear_logs();
    Ok(json!({ "success": true }))
}

#[tauri::command]
pub async fn clear_usage(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    state.request_log.clear_usage();
    Ok(json!({ "success": true }))
}

// 渲染层日志：统一送主进程写文件
#[tauri::command]
pub async fn log(level: String, message: String, data: Option<Value>) {
    logger::log(&level, &format!("[Renderer] {message}"), data.as_ref());
}
