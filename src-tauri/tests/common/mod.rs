#![allow(dead_code)]
// 测试共用的小工具：临时目录、配置构造、起 mock 上游、发请求。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;

use ccproxy_lib::config::{validate_config, AppConfig, ConfigManager};
use ccproxy_lib::request_log::RequestLog;
use ccproxy_lib::state::AppState;

static COUNTER: AtomicU64 = AtomicU64::new(0);

// 每个用例一个临时目录（写在系统 tmp 下，不污染项目）；Drop 时删掉
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&path).expect("建临时目录失败");
        Self { path }
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    pub fn write(&self, name: &str, content: &str) -> PathBuf {
        let target = self.join(name);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&target, content).expect("写文件失败");
        target
    }

    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// 走真实入口把 JSON 归一化成 AppConfig（与配置文件读进来时同一条路）
pub fn config_from(value: Value) -> AppConfig {
    let result = validate_config(&value);
    assert!(result.ok, "配置校验失败：{:?}", result.issues);
    result.config.expect("配置应可归一化")
}

// 测试要拿临时端口：真实配置不允许 0 端口，这里归一化之后再覆盖
pub fn config_with_port(value: Value, port: u16) -> AppConfig {
    let mut config = config_from(value);
    config.settings.port = port;
    config
}

pub fn test_state(config: AppConfig, dir: &TempDir) -> Arc<AppState> {
    let manager = ConfigManager::new(Some(dir.path.clone()));
    let request_log = RequestLog::new(Some(dir.path.clone()), None);
    let state = Arc::new(AppState::new(manager, request_log, config));
    state.proxy.init(state.clone());
    state
}

pub fn entry_value(state: &AppState, index: usize) -> Value {
    state.request_log.list().into_iter().nth(index).unwrap_or(Value::Null)
}

pub fn problems_contain(problems: &[String], needle: &str) -> bool {
    problems.iter().any(|item| item.contains(needle))
}

