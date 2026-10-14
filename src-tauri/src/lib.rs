// 应用入口：窗口 + 托盘 + 命令注册 + 代理生命周期。
// 关窗口只隐藏不退出（代理要继续服务），退出统一走托盘菜单。

pub mod clients;
pub mod commands;
pub mod config;
pub mod logger;
pub mod proxy;
pub mod request_log;
pub mod state;
pub mod tray;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::json;
use tauri::Manager;

use config::{create_client_token, ConfigManager};
use request_log::RequestLog;
use state::AppState;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// 与 Node 的 new Date().toISOString() 同形：2026-10-14T03:12:45.123Z
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn bootstrap(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    logger::init("/tmp/ccproxy");
    logger::info("[App] 启动中", None);

    let config_manager = ConfigManager::new(None);
    let loaded = config_manager.load();
    let mut config = loaded.config;
    let mut issues = loaded.issues;
    if loaded.recovered {
        logger::warn(
            "[Config] 配置已损坏并重建",
            Some(&json!({ "backupPath": loaded.backup_path, "reason": issues.join("；") })),
        );
    }
    // 客户端 token 是配置的必需项（检查客户端配置要用，之后校验入站请求也要用）。
    // 在内存里补一个并落盘：不落盘的话每次启动都会换一个新的，客户端那边填过也对不上
    let needs_client_token = config.settings.client_token.is_empty();
    if needs_client_token {
        config.settings.client_token = create_client_token();
    }
    // 首启与损坏重建时，拿到的默认配置只在内存里：必须落盘，否则下次启动还是走同一条分支
    if loaded.first_run || loaded.recovered || needs_client_token {
        let value = serde_json::to_value(&config)?;
        match config_manager.save(&value) {
            Ok((saved, extra)) => {
                config = saved;
                issues.extend(extra);
                logger::info(
                    "[Config] 已写入配置",
                    Some(&json!({
                        "path": config_manager.path(),
                        "firstRun": loaded.first_run,
                        "recovered": loaded.recovered,
                        "generatedClientToken": needs_client_token
                    })),
                );
            }
            Err(error) => {
                issues.push(format!("配置写入失败：{error}"));
                logger::error(
                    "[Config] 配置写入失败",
                    Some(&json!({ "path": config_manager.path(), "error": error })),
                );
            }
        }
    }
    logger::info(
        "[Config] 已加载",
        Some(&json!({
            "path": config_manager.path(),
            "port": config.settings.port,
            "issues": issues.len()
        })),
    );

    let request_log = RequestLog::new(Some(config_manager.dir()), None);
    let state = Arc::new(AppState::new(config_manager, request_log, config));
    state.set_handle(app.handle().clone());
    if let Ok(mut slot) = state.issues.write() {
        *slot = issues;
    }
    if let Ok(mut slot) = state.recovered.write() {
        *slot = loaded.recovered;
    }
    if let Ok(mut slot) = state.backup_path.write() {
        *slot = loaded.backup_path;
    }
    app.manage(state.clone());
    state.proxy.init(state.clone());

    // 启动时按配置同步一次副作用：用户可能在系统设置里关掉了自启，而配置里还写着 true
    let snapshot = state.config_snapshot();
    state.apply_config_effects(None, &snapshot);

    tray::create(app.handle(), &state)?;

    // 关窗口只隐藏，代理继续在后台服务
    let main_window = app.get_webview_window("main");
    logger::info(
        "[App] 窗口与托盘就绪",
        Some(&json!({
            "window": main_window.is_some(),
            "tray": app.tray_by_id("main").is_some(),
            "logPath": logger::path()
        })),
    );
    if let Some(window) = main_window {
        let handle = app.handle().clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::QUITTING.load(Ordering::SeqCst) {
                    return;
                }
                api.prevent_close();
                tray::hide_window(&handle);
            }
        });
    }

    let starter = state.clone();
    tauri::async_runtime::spawn(async move {
        starter.start_proxy().await;
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // 单实例：代理占固定端口，且托盘应用多开会同时出现两个图标。
    // 例外：设了 CCPROXY_ALLOW_MULTI=1 就不上这把锁——调试时可以在正式实例照常开着的情况下
    // 另起一个（同时用别的 HOME 指到别的配置，端口才不会撞）。日常运行不需要这个开关。
    let builder = if std::env::var("CCPROXY_ALLOW_MULTI").as_deref() == Ok("1") {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::get_config,
            commands::save_config,
            commands::regenerate_client_token,
            commands::get_client_dirs,
            commands::pick_client_dir,
            commands::add_client_dir,
            commands::apply_client_dir,
            commands::remove_client_dir,
            commands::get_proxy_status,
            commands::start_proxy,
            commands::stop_proxy,
            commands::restart_proxy,
            commands::fetch_provider_models,
            commands::get_logs,
            commands::get_usage,
            commands::get_storage_stats,
            commands::clear_logs,
            commands::clear_usage,
            commands::log
        ])
        .setup(|app| {
            bootstrap(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("CCProxy 启动失败")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // 窗口关掉不退出是上面的 hide 逻辑管的；真到了退出请求就放行
                tray::QUITTING.store(true, Ordering::SeqCst);
                let _ = app;
            }
        });
}

