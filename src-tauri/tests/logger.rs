// 日志单测：JSON Lines 格式、目录可切换、未初始化也能写

use serde_json::Value;

use ccproxy_lib::logger::Logger;

use common::TempDir;

mod common;

#[test]
fn writes_one_valid_json_line_per_call() {
    let dir = TempDir::new("ccproxy-log");
    let logger = Logger::new(dir.path.clone(), true);
    logger.initialize();
    logger.log("info", "[Proxy] 测试", Some(&serde_json::json!({ "port": 18888 })));

    let text = std::fs::read_to_string(logger.path()).unwrap();
    let lines: Vec<&str> = text.trim().split('\n').collect();
    assert_eq!(lines.len(), 1);
    let entry: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(entry["level"], "info");
    assert_eq!(entry["message"], "[Proxy] 测试");
    assert_eq!(entry["data"]["port"], 18888);
    assert!(entry["timestamp"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn writes_without_explicit_initialize() {
    let dir = TempDir::new("ccproxy-log");
    let nested = dir.join("nested");
    let logger = Logger::new(nested, true);
    logger.log("warn", "未初始化", None);
    assert!(logger.path().exists());
}

#[test]
fn separate_loggers_write_to_their_own_dirs() {
    let first = TempDir::new("ccproxy-log-a");
    let second = TempDir::new("ccproxy-log-b");
    Logger::new(first.path.clone(), true).log("error", "第一条", None);
    Logger::new(second.path.clone(), true).log("error", "第二条", None);
    assert!(std::fs::read_to_string(first.join("app.log")).unwrap().contains("第一条"));
    assert!(std::fs::read_to_string(second.join("app.log")).unwrap().contains("第二条"));
}

#[test]
fn no_data_field_when_data_is_absent() {
    let dir = TempDir::new("ccproxy-log");
    let logger = Logger::new(dir.path.clone(), true);
    logger.log("error", "无数据", None);
    let text = std::fs::read_to_string(logger.path()).unwrap();
    let entry: Value = serde_json::from_str(text.trim().split('\n').next().unwrap()).unwrap();
    assert!(entry.get("data").is_none());
}

