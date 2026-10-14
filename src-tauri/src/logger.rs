// 应用日志：JSON Lines 写 /tmp/ccproxy/app.log，格式与原 Electron 版同构。
// 日志是旁路：写失败绝不能影响代理主流程。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};

pub const DEBUG: &str = "debug";
pub const INFO: &str = "info";
pub const WARN: &str = "warn";
pub const ERROR: &str = "error";

pub struct Logger {
    base_dir: PathBuf,
    log_path: PathBuf,
    silent: bool,
}

impl Logger {
    pub fn new(base_dir: impl Into<PathBuf>, silent: bool) -> Self {
        let base_dir = base_dir.into();
        let log_path = base_dir.join("app.log");
        Self { base_dir, log_path, silent }
    }

    pub fn initialize(&self) -> bool {
        fs::create_dir_all(&self.base_dir).is_ok()
    }

    pub fn path(&self) -> &Path {
        &self.log_path
    }

    pub fn log(&self, level: &str, message: &str, data: Option<&Value>) -> Value {
        let timestamp = crate::now_iso();
        let mut entry = json!({ "timestamp": timestamp, "level": level, "message": message });
        if let Some(data) = data {
            entry["data"] = data.clone();
        }
        if let Ok(line) = serde_json::to_string(&entry) {
            if fs::create_dir_all(&self.base_dir).is_ok() {
                if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&self.log_path) {
                    let _ = writeln!(file, "{line}");
                }
            }
        }
        if !self.silent {
            let suffix = match data {
                Some(data) => format!(" {}", serde_json::to_string(data).unwrap_or_default()),
                None => String::new(),
            };
            println!("[{timestamp}] {} {message}{suffix}", level.to_uppercase());
        }
        entry
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn logger() -> &'static Logger {
    LOGGER.get_or_init(|| Logger::new("/tmp/ccproxy", false))
}

// 启动阶段调用一次，把日志目录定下来；之后各处用自由函数写日志
pub fn init(base_dir: impl Into<PathBuf>) {
    let _ = LOGGER.set(Logger::new(base_dir, false));
}

pub fn path() -> PathBuf {
    logger().path().to_path_buf()
}

pub fn log(level: &str, message: &str, data: Option<&Value>) {
    logger().log(level, message, data);
}

pub fn debug(message: &str, data: Option<&Value>) {
    log(DEBUG, message, data);
}

pub fn info(message: &str, data: Option<&Value>) {
    log(INFO, message, data);
}

pub fn warn(message: &str, data: Option<&Value>) {
    log(WARN, message, data);
}

pub fn error(message: &str, data: Option<&Value>) {
    log(ERROR, message, data);
}

