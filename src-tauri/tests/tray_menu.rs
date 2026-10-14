// 托盘状态行文案单测：状态行含端口 / 错误信息，启停文案随状态翻转

use ccproxy_lib::state::ProxyStatus;
use ccproxy_lib::tray::status_label;

fn status(name: &str, port: Option<u16>, error: Option<&str>) -> ProxyStatus {
    ProxyStatus {
        status: name.to_string(),
        port,
        error: error.map(str::to_string),
    }
}

#[test]
fn running_label_contains_port() {
    let label = status_label(&status("running", Some(18888), None));
    assert_eq!(label, "运行中 · 127.0.0.1:18888");
}

#[test]
fn error_label_contains_reason() {
    assert_eq!(
        status_label(&status("error", Some(18888), Some("EADDRINUSE"))),
        "启动失败：EADDRINUSE"
    );
    assert_eq!(status_label(&status("error", Some(18888), None)), "启动失败");
}

#[test]
fn stopped_label_is_plain() {
    assert_eq!(status_label(&status("stopped", Some(18888), None)), "已停止");
}

#[test]
fn labels_never_use_dot_characters() {
    // 状态靠图标图片表状态，不再用 ● / ○ 字符
    for label in [
        status_label(&status("running", Some(18888), None)),
        status_label(&status("error", Some(18888), Some("EADDRINUSE"))),
        status_label(&status("stopped", Some(18888), None)),
    ] {
        assert!(!label.contains('●') && !label.contains('○'));
    }
}

