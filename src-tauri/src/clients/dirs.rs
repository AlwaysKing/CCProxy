// 客户端配置目录：内置位置、目录身份判定、扫描。

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::ClientDir;

// codex 的特征键。config.toml 这名字撞车太多（Rust 的 Cargo 配置就叫这个），
// 必须在内容里看到 codex 特有的键才认
fn has_codex_mark(text: &str) -> bool {
    if text.contains("[model_providers") {
        return true;
    }
    const NEEDLE: &str = "model_provider";
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = text[from..].find(NEEDLE) {
        let index = from + offset;
        // 行首到关键字之间只能有空白
        let line_start = match text[..index].rfind('\n') {
            Some(position) => position + 1,
            None => 0,
        };
        let prefix_ok = bytes[line_start..index].iter().all(|byte| byte.is_ascii_whitespace());
        let after = &bytes[index + NEEDLE.len()..];
        let whitespace = after.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
        if prefix_ok && after.get(whitespace) == Some(&b'=') {
            return true;
        }
        from = index + 1;
    }
    false
}

fn file_exists(target: &Path) -> bool {
    fs::metadata(target).map(|meta| meta.is_file()).unwrap_or(false)
}

fn is_claude_dir(dir_path: &Path) -> bool {
    file_exists(&dir_path.join("settings.json"))
}

fn is_codex_dir(dir_path: &Path) -> bool {
    let config_path = dir_path.join("config.toml");
    if !file_exists(&config_path) {
        return false;
    }
    fs::read_to_string(&config_path).map(|text| has_codex_mark(&text)).unwrap_or(false)
}

// 判定目录是哪家的配置目录：settings.json → claude，config.toml（含 codex 特征键）→ codex，
// 都不是返回 None。两个都在的按 claude 算——claude 的标志文件不撞名，先信它
pub fn detect_dir_kind(dir_path: &str) -> Option<String> {
    if dir_path.trim().is_empty() {
        return None;
    }
    let path = PathBuf::from(dir_path);
    if is_claude_dir(&path) {
        return Some("claude".to_string());
    }
    if is_codex_dir(&path) {
        return Some("codex".to_string());
    }
    None
}

// 内置扫描位置：前两个是 CLI 默认家目录，后两个是 Xcode 内置 Coding Assistant 的。
// kind 是「预期身份」：路径存在但内容不是这一家的不算数
pub fn builtin_dirs(home: Option<&Path>) -> Vec<ClientDir> {
    let base = home.map(PathBuf::from).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
    let make = |kind: &str, path: PathBuf| ClientDir {
        id: String::new(),
        kind: kind.to_string(),
        path: path.to_string_lossy().to_string(),
    };
    vec![
        make("claude", base.join(".claude")),
        make("codex", base.join(".codex")),
        make(
            "claude",
            base.join("Library").join("Developer").join("Xcode").join("CodingAssistant").join("ClaudeAgentConfig"),
        ),
        make("codex", base.join("Library").join("Developer").join("Xcode").join("CodingAssistant").join("codex")),
    ]
}

pub struct ScanResult {
    pub builtin: Vec<ClientDir>,
    pub manual: Vec<ClientDir>,
    pub dropped: Vec<ClientDir>,
}

// 扫一遍：内置的只收「路径存在且身份与预期一致」的；手动的逐条实测，不再是配置目录的
// 挑出来交给调用方删。手动条目的 kind 以实测为准——存的时候是 codex、后来目录长成了
// claude 的样子，就按 claude 返回，调用方把矫正落回配置
pub fn scan_client_dirs(home: Option<&Path>, manual: &[ClientDir]) -> ScanResult {
    let mut builtin = Vec::new();
    for entry in builtin_dirs(home) {
        if detect_dir_kind(&entry.path).as_deref() == Some(entry.kind.as_str()) {
            builtin.push(entry);
        }
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for entry in manual {
        match detect_dir_kind(&entry.path) {
            None => dropped.push(entry.clone()),
            Some(kind) => {
                if kind == entry.kind {
                    kept.push(entry.clone());
                } else {
                    let mut fixed = entry.clone();
                    fixed.kind = kind;
                    kept.push(fixed);
                }
            }
        }
    }

    ScanResult { builtin, manual: kept, dropped }
}

// path.resolve 的等价物：相对路径按 base 展开，再去掉 . 与 ..
pub fn resolve_path(base: &str, name: &str) -> String {
    let path = if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        Path::new(base).join(name)
    };
    normalize_path(&path).to_string_lossy().to_string()
}

pub fn resolve_single(name: &str) -> String {
    let path = if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(name)
    };
    normalize_path(&path).to_string_lossy().to_string()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

