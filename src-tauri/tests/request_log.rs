// 请求日志存储单测：环形缓冲上限、两份落盘的分目录、用量聚合、清理语义互不影响

use serde_json::{json, Value};

use ccproxy_lib::request_log::{date_key, RequestLog, MAX_ENTRIES};

use common::TempDir;

mod common;

fn log_in(dir: &TempDir, max_entries: Option<usize>) -> RequestLog {
    RequestLog::new(Some(dir.path.clone()), max_entries)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// 一条正常转发成功的 entry，字段按 server 的日志形态给足
fn forwarded(fields: Value) -> Value {
    let mut base = json!({
        "id": "x",
        "timestamp": now_ms(),
        "method": "POST",
        "path": "/v1/messages",
        "protocol": "anthropic",
        "clientModel": "claude-*",
        "matchKind": "wildcard",
        "matchedRule": "claude-*",
        "providerId": "p1",
        "providerName": "供应商一",
        "realModel": "glm-4.6",
        "statusCode": 200,
        "outcome": "forwarded",
        "errorCode": null,
        "error": null,
        "cancelled": false,
        "tokens": { "input": 100, "output": 20 }
    });
    if let (Some(target), Some(source)) = (base.as_object_mut(), fields.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    base
}

fn read_lines(path: std::path::PathBuf) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn memory_buffer_keeps_limit_and_puts_newest_first() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, Some(3));
    for index in 0..5 {
        log.record(&forwarded(json!({ "id": format!("e{index}") })), true, true);
    }
    let ids: Vec<String> = log
        .list()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, vec!["e4", "e3", "e2"]);
    assert_eq!(MAX_ENTRIES, 500);
}

#[test]
fn only_forwarded_requests_reach_the_usage_dir() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    log.record(&forwarded(json!({})), true, true);
    log.record(&forwarded(json!({ "statusCode": 500, "tokens": null })), true, true);
    log.record(
        &forwarded(json!({ "outcome": "rejected", "statusCode": 404, "providerId": null, "providerName": null, "realModel": null, "tokens": null })),
        true,
        true,
    );
    log.record(
        &forwarded(json!({ "outcome": "detect_failed", "protocol": null, "providerId": null, "providerName": null, "realModel": null, "tokens": null })),
        true,
        true,
    );

    let usage = log.usage(&json!({}));
    assert_eq!(usage["totals"]["requests"], json!(2));
    assert_eq!(usage["totals"]["failed"], json!(1));

    let day = date_key(now_ms());
    let log_lines = read_lines(dir.path.join("logs").join(format!("{day}.jsonl")));
    let usage_lines = read_lines(dir.path.join("usage").join(format!("{day}.jsonl")));
    assert_eq!(log_lines.len(), 4);
    assert_eq!(usage_lines.len(), 2);
}

#[test]
fn aggregation_buckets_by_day_protocol_provider_model_and_cancel_is_not_failure() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    let now = now_ms();
    log.record(
        &forwarded(json!({ "timestamp": now, "protocol": "anthropic", "providerName": "A", "realModel": "glm-4.6", "tokens": { "input": 100, "output": 50, "cacheRead": 1000 } })),
        true,
        true,
    );
    log.record(
        &forwarded(json!({ "timestamp": now, "protocol": "openai", "providerName": "B", "realModel": "glm-4.6", "tokens": { "input": 10, "output": 5 } })),
        true,
        true,
    );
    log.record(
        &forwarded(json!({ "timestamp": now, "protocol": "openai", "providerName": "B", "realModel": "kimi", "cancelled": true, "tokens": { "input": 1, "output": 1 } })),
        true,
        true,
    );

    let usage = log.usage(&json!({}));
    assert_eq!(usage["totals"]["requests"], json!(3));
    assert_eq!(usage["totals"]["failed"], json!(0));
    assert_eq!(usage["totals"]["cancelled"], json!(1));
    assert_eq!(usage["totals"]["input"], json!(111));
    assert_eq!(usage["totals"]["output"], json!(56));
    assert_eq!(usage["totals"]["cacheRead"], json!(1000));

    let find = |key: &str, bucket: &str| -> Value {
        usage[bucket]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["key"] == json!(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(find("anthropic", "byProtocol")["output"], json!(50));
    assert_eq!(find("anthropic", "byProtocol")["cacheRead"], json!(1000));
    assert_eq!(find("B", "byProvider")["requests"], json!(2));
    // 没上报缓存的记录按 0 计入，不会把整桶变成 null
    assert_eq!(find("B", "byProvider")["cacheRead"], json!(0));
    // 模型维度把两个协议的 glm-4.6 合在一起：同一个模型走哪个协议都是同一个模型
    assert_eq!(find("glm-4.6", "byModel")["requests"], json!(2));
    assert_eq!(find("glm-4.6", "byModel")["input"], json!(110));
    assert_eq!(usage["byDay"].as_array().unwrap().len(), 1);
    assert_eq!(usage["byDay"][0]["key"], json!(date_key(now)));
}

#[test]
fn days_one_covers_today_only() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    let today = now_ms();
    let yesterday = today - 24 * 60 * 60 * 1000;
    log.record(&forwarded(json!({ "timestamp": yesterday, "tokens": { "input": 1, "output": 1 } })), true, true);
    log.record(&forwarded(json!({ "timestamp": today, "tokens": { "input": 10, "output": 10 } })), true, true);

    let names: Vec<String> = std::fs::read_dir(dir.path.join("usage"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".jsonl"))
        .collect();
    // 昨天与今天各一个日文件（跨日时可能是同一天，至少有一个）
    assert!(!names.is_empty());

    let today_only = log.usage(&json!({ "days": 1 }));
    let expected_today = if date_key(yesterday) == date_key(today) { 2 } else { 1 };
    assert_eq!(today_only["totals"]["requests"], json!(expected_today));

    let all = log.usage(&json!({}));
    assert_eq!(all["totals"]["requests"], json!(2));
}

#[test]
fn broken_lines_are_skipped_without_losing_the_day() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    log.record(&forwarded(json!({ "tokens": { "input": 5, "output": 5 } })), true, true);
    let day_file = dir.path.join("usage").join(format!("{}.jsonl", date_key(now_ms())));
    let mut text = std::fs::read_to_string(&day_file).unwrap();
    // 合法 JSON 但缺 timestamp 的行（手工编辑才会有）也要跳过，别聚出 "NaN-NaN-NaN" 桶
    text.push_str("不是JSON\n{也是坏的\n");
    text.push_str(&serde_json::to_string(&json!({ "realModel": "glm-4.6" })).unwrap());
    text.push('\n');
    std::fs::write(&day_file, text).unwrap();

    let usage = log.usage(&json!({}));
    assert_eq!(usage["totals"]["requests"], json!(1));
    assert_eq!(usage["totals"]["input"], json!(5));
}

#[test]
fn clearing_logs_and_usage_do_not_affect_each_other() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    log.record(&forwarded(json!({})), true, true);
    assert!(!std::fs::read_dir(dir.path.join("logs")).unwrap().next().is_none());
    assert!(!std::fs::read_dir(dir.path.join("usage")).unwrap().next().is_none());

    log.clear_logs();
    assert!(log.list().is_empty());
    assert!(std::fs::read_dir(dir.path.join("logs")).unwrap().next().is_none());
    assert!(!std::fs::read_dir(dir.path.join("usage")).unwrap().next().is_none());

    log.clear_usage();
    assert!(std::fs::read_dir(dir.path.join("usage")).unwrap().next().is_none());
    assert_eq!(log.usage(&json!({}))["totals"]["requests"], json!(0));
}

#[test]
fn stats_counts_files_and_bytes() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    log.record(&forwarded(json!({})), true, true);
    let stats = log.stats();
    assert_eq!(stats["logs"]["files"], json!(1));
    assert_eq!(stats["usage"]["files"], json!(1));
    assert!(stats["logs"]["bytes"].as_i64().unwrap() > 0);
    assert!(stats["usage"]["bytes"].as_i64().unwrap() > 0);
    // 用量行字段更少，必然更小
    assert!(stats["usage"]["bytes"].as_i64().unwrap() < stats["logs"]["bytes"].as_i64().unwrap());
}

#[test]
fn record_respects_both_switches() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    // 只开日志：进内存与 logs/，不写 usage/
    log.record(&forwarded(json!({ "id": "only-logs" })), true, false);
    // 只开用量：写 usage/，不进内存也不写 logs/
    log.record(&forwarded(json!({ "id": "only-usage" })), false, true);

    let ids: Vec<String> = log
        .list()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, vec!["only-logs"]);
    let day = date_key(now_ms());
    assert_eq!(read_lines(dir.path.join("logs").join(format!("{day}.jsonl"))).len(), 1);
    assert_eq!(read_lines(dir.path.join("usage").join(format!("{day}.jsonl"))).len(), 1);
    assert_eq!(log.usage(&json!({}))["totals"]["requests"], json!(1));
}

#[test]
fn both_switches_off_writes_nothing() {
    let dir = TempDir::new("ccproxy-log");
    let log = log_in(&dir, None);
    log.record(&forwarded(json!({})), false, false);
    assert!(log.list().is_empty());
    assert!(!dir.path.join("logs").exists());
    assert!(!dir.path.join("usage").exists());
}

