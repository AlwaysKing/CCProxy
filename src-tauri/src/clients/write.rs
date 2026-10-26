// 客户端配置的共享写入底座：原子替换、软链跟随、权限沿用和覆盖式备份。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

const BACKUP_SUFFIX: &str = ".ccproxy-bak";
const TEMP_SUFFIX: &str = ".ccproxy-tmp";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub file: String,
    pub backup_path: Option<String>,
    pub changed: bool,
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or(0o600)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> u32 {
    0o600
}

fn write_file_atomic(target_path: &Path, content: &str) -> Result<(), String> {
    let mode = file_mode(target_path);
    let temp_path = PathBuf::from(format!("{}{TEMP_SUFFIX}", target_path.to_string_lossy()));
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, content).map_err(|error| error.to_string())?;
    set_mode(&temp_path, mode);
    fs::rename(&temp_path, target_path).map_err(|error| error.to_string())
}

// dotfiles 常把配置文件做成软链。rename 会把链接换成普通文件，所以命中软链时改写真实目标。
fn resolve_target(file_path: &Path) -> PathBuf {
    if fs::symlink_metadata(file_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        if let Ok(real) = fs::canonicalize(file_path) {
            return real;
        }
    }
    file_path.to_path_buf()
}

fn backup_file(target_path: &Path) -> Option<String> {
    if !target_path.exists() {
        return None;
    }
    let backup_path = PathBuf::from(format!("{}{BACKUP_SUFFIX}", target_path.to_string_lossy()));
    fs::copy(target_path, &backup_path).ok()?;
    Some(backup_path.to_string_lossy().to_string())
}

pub fn write_if_changed(
    file_path: &Path,
    text: &str,
    changed: bool,
) -> Result<WriteResult, String> {
    let file = file_path.to_string_lossy().to_string();
    if !changed {
        return Ok(WriteResult {
            file,
            backup_path: None,
            changed: false,
        });
    }
    let target = resolve_target(file_path);
    let backup_path = backup_file(&target);
    write_file_atomic(&target, text)?;
    Ok(WriteResult {
        file,
        backup_path,
        changed: true,
    })
}
