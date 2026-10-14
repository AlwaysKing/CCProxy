// 托盘：图标反映代理运行状态，菜单给打开窗口 / 启停代理 / 退出三个落点。
// 状态或配置变化后由 AppState 调 refresh() 重建菜单与图标。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{IconMenuItem, Menu, MenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::logger;
use crate::state::{AppState, ProxyStatus};

pub static QUITTING: AtomicBool = AtomicBool::new(false);

const TRAY_ID: &str = "main";

// 菜单栏那个图标：macOS 模板图（纯黑 + alpha），系统按深浅色自动反色。
// 用 @2x 那张当源：底层会把显示尺寸统一压到 18pt，源分辨率高一些在视网膜屏上才不糊
const ICON_RUNNING: &[u8] = include_bytes!("../../resources/icons/trayRunningTemplate@2x.png");
const ICON_STOPPED: &[u8] = include_bytes!("../../resources/icons/trayStoppedTemplate@2x.png");

// 菜单里那枚圆点的颜色：中灰，深浅两种菜单背景上都看得见
const MENU_DOT_GRAY: u8 = 142;

// running 之外（stopped / error）都用停止态那枚空心圆
fn status_icon(status: &str) -> Option<Image<'static>> {
    let bytes = if status == "running" { ICON_RUNNING } else { ICON_STOPPED };
    Image::from_bytes(bytes).ok()
}

// 托盘菜单里状态行那枚圆点。Tauri 的菜单封装不给菜单项图标设「模板」标记（底层是 muda，
// 它只是把图设进 NSMenuItem），纯黑图会被原样画在深色菜单上，等于看不见。
// 这里把模板图的 alpha 原样留下、颜色换成中灰：形状与菜单栏那枚完全一致，两种背景都看得清
fn menu_dot(status: &str) -> Option<Image<'static>> {
    let source = status_icon(status)?;
    let (width, height) = (source.width(), source.height());
    let mut rgba = source.rgba().to_vec();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[0] = MENU_DOT_GRAY;
        pixel[1] = MENU_DOT_GRAY;
        pixel[2] = MENU_DOT_GRAY;
    }
    Some(Image::new_owned(rgba, width, height))
}

pub fn status_label(proxy: &ProxyStatus) -> String {
    if proxy.status == "running" {
        return format!("运行中 · 127.0.0.1:{}", proxy.port.unwrap_or(0));
    }
    let base = if proxy.status == "error" { "启动失败" } else { "已停止" };
    match &proxy.error {
        Some(error) if !error.is_empty() => format!("{base}：{error}"),
        _ => base.to_string(),
    }
}

// 菜单状态行的圆点与托盘图标共用同一套模板图，两处形状口径才不会各画一套
fn build_menu(app: &AppHandle, status: &ProxyStatus) -> tauri::Result<Menu<tauri::Wry>> {
    let running = status.status == "running";
    let status_item = IconMenuItem::with_id(app, "status", status_label(status), false, menu_dot(&status.status), None::<&str>)?;
    MenuBuilder::new(app)
        .item(&status_item)
        .separator()
        .text("open", "打开主窗口")
        .text("toggle", if running { "停止代理" } else { "启动代理" })
        .separator()
        .text("quit", "退出 CCProxy")
        .build()
}

pub fn create(app: &AppHandle, state: &Arc<AppState>) -> tauri::Result<()> {
    let status = state.proxy_status();
    let menu = build_menu(app, &status)?;
    let icon = status_icon(&status.status);
    if icon.is_none() {
        // 图标读不出来时托盘会退回系统默认图，这里留一条日志，免得只看到「图标不对」查不到原因
        logger::warn("[Tray] 托盘模板图加载失败，将使用系统默认图标", None);
    }
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("CCProxy")
        .icon_as_template(true)
        .menu(&menu)
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    // 建好之后再统一走一次 refresh：图标与「模板」标记必须一起设（见 refresh 里的说明）
    refresh(state);
    Ok(())
}

// 状态一变就重建菜单
pub fn refresh(state: &AppState) {
    let Some(app) = state.handle() else { return };
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let status = state.proxy_status();
    if let Ok(menu) = build_menu(app, &status) {
        let _ = tray.set_menu(Some(menu));
    }
    if let Some(icon) = status_icon(&status.status) {
        // 必须用 set_icon_with_as_template：单独调 set_icon 会新建一张 NSImage 并丢掉模板标记，
        // 于是深色菜单栏里那枚图标会被原样画成黑色，而不是反色成白色
        let _ = tray.set_icon_with_as_template(Some(icon), true);
    }
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "open" => show_window(app),
        "toggle" => {
            let state = app.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                state.toggle_proxy().await;
            });
        }
        "quit" => {
            QUITTING.store(true, Ordering::SeqCst);
            app.exit(0);
        }
        _ => {}
    }
}

// 关窗口只隐藏：代理要继续在后台服务。同时隐藏 Dock 图标（agent 应用模式），
// 看起来像已退出，实际进程和托盘还活着；show_window() 时再恢复
pub fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

pub fn show_window(app: &AppHandle) {
    // 恢复 Dock 图标（关窗口时被隐藏了）再显示窗口，否则窗口没有所属的 Dock 入口
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

