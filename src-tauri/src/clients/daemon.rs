// codex 的 app-server daemon 体检：它启动时把 config.toml 与模型清单（models.json）读进内存，
// 之后磁盘上这两份文件被改它也不会重读——用户看到的是「改了没生效」。
// 这里从 pid 文件认出正在跑的 daemon，界面据此提醒执行 `codex app-server daemon restart`。
//
// 不走 `codex app-server daemon version`：从访达启动的 App 里 PATH 通常没有 homebrew，
// 而 pid 文件就在 codex 目录下，认得它就不需要外部命令，离线也能判断。

use std::fs;
use std::path::Path;

use serde::Serialize;

pub const DAEMON_DIR: &str = "app-server-daemon";
pub const DAEMON_PID_FILE: &str = "daemon.pid";
pub const RESTART_COMMAND: &str = "codex app-server daemon restart";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDaemon {
    pub running: bool,
    pub pid: Option<u32>,
    pub dir_path: String,
    pub restart_command: &'static str,
}

// daemon.pid 是 {"pid":123,...} 形式的 JSON（还带进程启动时刻与可执行文件指纹，
// 那些是 codex 自己防 pid 复用用的，这里用不上）；早期版本写的是纯数字，两种都认
fn parse_pid(text: &str) -> Option<u32> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(pid) = value.get("pid").and_then(serde_json::Value::as_u64) {
            return u32::try_from(pid).ok();
        }
    }
    text.trim().parse::<u32>().ok()
}

// 探活：daemon 只在启动时写一次 pid 文件，异常退出后它可能留在盘上，只看文件在不在会误报。
// 信号 0 只做存在性与权限判定，不打扰那个进程
#[cfg(unix)]
fn process_alive(pid: u32) -> Option<bool> {
    // 进程没了的时候 kill 会往 stderr 吐一行「No such process」，那是预期内的答案，别让它漏到日志里
    let status = std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    Some(status.success())
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> Option<bool> {
    None
}

// 体检一个 codex 目录。探不出死活（非 unix、没有 /bin/kill）时按 pid 文件在不在算，
// 宁可多提醒一次：真没跑 daemon 时用户执行 restart 也只是把它起起来
pub fn inspect(dir_path: &str) -> CodexDaemon {
    let pid = fs::read_to_string(Path::new(dir_path).join(DAEMON_DIR).join(DAEMON_PID_FILE))
        .ok()
        .as_deref()
        .and_then(parse_pid);
    let running = match pid.map(process_alive) {
        Some(Some(alive)) => alive,
        _ => pid.is_some(),
    };
    CodexDaemon {
        running,
        pid: if running { pid } else { None },
        dir_path: dir_path.to_string(),
        restart_command: RESTART_COMMAND,
    }
}
