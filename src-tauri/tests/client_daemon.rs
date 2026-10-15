// codex app-server daemon 体检单测：认 pid 文件、认死活，误报要少

use ccproxy_lib::clients::daemon::{inspect, DAEMON_DIR, DAEMON_PID_FILE, RESTART_COMMAND};

use common::TempDir;

mod common;

fn write_pid(dir: &TempDir, content: &str) {
    dir.write(&format!("{DAEMON_DIR}/{DAEMON_PID_FILE}"), content);
}

fn path_of(dir: &TempDir) -> String {
    dir.path.to_string_lossy().to_string()
}

#[test]
fn missing_pid_file_is_not_running() {
    let dir = TempDir::new("ccproxy-daemon");
    let status = inspect(&path_of(&dir));
    assert!(!status.running);
    assert_eq!(status.pid, None);
    assert_eq!(status.restart_command, RESTART_COMMAND);
}

#[test]
fn live_pid_is_running() {
    let dir = TempDir::new("ccproxy-daemon");
    let pid = std::process::id();
    write_pid(
        &dir,
        &format!("{{\"pid\":{pid},\"processStartTime\":\"Thu Oct 15 14:45:10 2026\"}}"),
    );
    let status = inspect(&path_of(&dir));
    assert!(status.running);
    assert_eq!(status.pid, Some(pid));
}

// pid 文件里的这个进程早就没了（daemon 异常退出时 pid 文件会赖在盘上），不能算在跑
#[test]
fn stale_pid_file_is_not_running() {
    let dir = TempDir::new("ccproxy-daemon");
    // macOS 的 pid 上限是 99998，这个大数不会撞上真进程
    write_pid(&dir, "{\"pid\":999999,\"processStartTime\":\"Thu Oct 15 14:45:10 2026\"}");
    let status = inspect(&path_of(&dir));
    assert!(!status.running);
    assert_eq!(status.pid, None);
}

// 早期版本写的是纯数字
#[test]
fn plain_number_pid_file_is_understood() {
    let dir = TempDir::new("ccproxy-daemon");
    write_pid(&dir, &format!("{}\n", std::process::id()));
    let status = inspect(&path_of(&dir));
    assert!(status.running);
    assert_eq!(status.pid, Some(std::process::id()));
}

#[test]
fn broken_pid_file_is_not_running() {
    let dir = TempDir::new("ccproxy-daemon");
    write_pid(&dir, "not a pid");
    assert!(!inspect(&path_of(&dir)).running);
}
