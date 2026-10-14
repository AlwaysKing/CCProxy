// 请求日志存储：内存环形缓冲（日志页实时看）+ 两份按天 JSONL 落盘。
// 两份是有意分开的：logs/ 存全字段供事后排查，usage/ 只存聚合用得上的字段。
// 分开的理由是清理语义不同——清日志不该连用量历史一起清掉，反之亦然。
//
// 落盘是旁路：目录建不起来、磁盘写不进去都不能影响代理主流程；读的时候坏行直接跳过。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{Datelike, Local, TimeZone};
use serde_json::{json, Value};

pub const MAX_ENTRIES: usize = 500;

// 按本地日期切文件：用户说的「今天」是本地时间的今天，按 UTC 切会在跨日时错位一格
pub fn date_key(timestamp_ms: i64) -> String {
    match Local.timestamp_millis_opt(timestamp_ms).single() {
        Some(time) => format!("{:04}-{:02}-{:02}", time.year(), time.month(), time.day()),
        None => String::new(),
    }
}

fn today_key() -> String {
    date_key(Local::now().timestamp_millis())
}

fn shift_days(key: &str, delta: i64) -> String {
    let parts: Vec<i64> = key.split('-').filter_map(|part| part.parse().ok()).collect();
    if parts.len() != 3 {
        return key.to_string();
    }
    let Some(base) = Local.with_ymd_and_hms(parts[0] as i32, parts[1] as u32, parts[2] as u32, 12, 0, 0).single()
    else {
        return key.to_string();
    };
    date_key((base + chrono::Duration::days(delta)).timestamp_millis())
}

fn is_day_file(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != 16 {
        return false;
    }
    // 只看 YYYY-MM-DD 这 10 个字符，后面是 ".jsonl"
    for (index, byte) in bytes.iter().take(10).enumerate() {
        let ok = match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        };
        if !ok {
            return false;
        }
    }
    name.ends_with(".jsonl")
}

// 用量记录只留聚合用得上的字段，也够看单条的消耗
fn to_usage_record(entry: &Value) -> Value {
    let cancelled = entry.get("cancelled").and_then(Value::as_bool) == Some(true);
    let status_code = entry.get("statusCode").and_then(Value::as_i64).unwrap_or(0);
    let status = if cancelled {
        "cancelled"
    } else if (200..300).contains(&status_code) {
        "ok"
    } else {
        "failed"
    };
    let tokens = entry.get("tokens").filter(|value| !value.is_null());
    json!({
        "timestamp": entry.get("timestamp").cloned().unwrap_or(Value::Null),
        "protocol": entry.get("protocol").cloned().unwrap_or(Value::Null),
        "providerId": entry.get("providerId").cloned().unwrap_or(Value::Null),
        "providerName": entry.get("providerName").cloned().unwrap_or(Value::Null),
        "clientModel": entry.get("clientModel").cloned().unwrap_or(Value::Null),
        "realModel": entry.get("realModel").cloned().unwrap_or(Value::Null),
        "status": status,
        "input": tokens.and_then(|value| value.get("input")).cloned().unwrap_or(Value::Null),
        "output": tokens.and_then(|value| value.get("output")).cloned().unwrap_or(Value::Null)
    })
}

#[derive(Clone, Copy, Default)]
struct Bucket {
    requests: i64,
    failed: i64,
    cancelled: i64,
    input: i64,
    output: i64,
}

impl Bucket {
    fn to_json(&self) -> Value {
        json!({
            "requests": self.requests,
            "failed": self.failed,
            "cancelled": self.cancelled,
            "input": self.input,
            "output": self.output
        })
    }
}

fn number_or_null(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(int) = value.as_i64() {
        return Some(int);
    }
    value.as_f64().map(|float| float as i64)
}

// 取消不算失败：用户自己按了 Esc 不该污染上游错误率
fn add_to_bucket(bucket: &mut Bucket, record: &Value) {
    bucket.requests += 1;
    let status = record.get("status").and_then(Value::as_str).unwrap_or("");
    if status == "cancelled" {
        bucket.cancelled += 1;
    } else if status != "ok" {
        bucket.failed += 1;
    }
    if let Some(input) = number_or_null(record.get("input")) {
        bucket.input += input;
    }
    if let Some(output) = number_or_null(record.get("output")) {
        bucket.output += output;
    }
}

fn text_or(value: &Value, fallback: &str) -> String {
    match value.as_str() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => fallback.to_string(),
    }
}

struct Range {
    from: Option<String>,
    to: Option<String>,
}

// 区间两种给法：显式 from/to（YYYY-MM-DD），或只给 days（含今天在内的最近 N 天）。
// 日期在这里算而不是让界面算：按本地日期切文件的是这里，两边各算一次容易错开一天
fn resolve_range(options: &Value) -> Range {
    let from = options.get("from").and_then(Value::as_str).filter(|text| !text.is_empty());
    let to = options.get("to").and_then(Value::as_str).filter(|text| !text.is_empty());
    if from.is_some() || to.is_some() {
        return Range { from: from.map(str::to_string), to: to.map(str::to_string) };
    }
    let days = options.get("days").and_then(Value::as_i64);
    let Some(days) = days.filter(|days| *days >= 1) else {
        return Range { from: None, to: None };
    };
    let end = today_key();
    Range { from: Some(shift_days(&end, -(days - 1))), to: Some(end) }
}

pub struct RequestLog {
    base_dir: PathBuf,
    logs_dir: PathBuf,
    usage_dir: PathBuf,
    limit: usize,
    entries: Mutex<Vec<Value>>,
}

impl RequestLog {
    pub fn new(dir: Option<PathBuf>, max_entries: Option<usize>) -> Self {
        let base_dir = dir.unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".ccproxy"));
        Self {
            logs_dir: base_dir.join("logs"),
            usage_dir: base_dir.join("usage"),
            limit: max_entries.filter(|limit| *limit > 0).unwrap_or(MAX_ENTRIES),
            entries: Mutex::new(Vec::new()),
            base_dir,
        }
    }

    fn append_line(&self, target: &Path, payload: &Value) {
        let timestamp = payload.get("timestamp").and_then(Value::as_i64).unwrap_or_else(|| Local::now().timestamp_millis());
        if fs::create_dir_all(target).is_err() {
            return;
        }
        let Ok(line) = serde_json::to_string(payload) else {
            return;
        };
        let path = target.join(format!("{}.jsonl", date_key(timestamp)));
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
    }

    fn day_files(&self, target: &Path) -> Vec<String> {
        let Ok(read) = fs::read_dir(target) else {
            return Vec::new();
        };
        let mut names: Vec<String> = read
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| is_day_file(name))
            .collect();
        names.sort();
        names
    }

    // 日志与用量各有一道开关：关了那一项就整条不落（内存也不留）
    pub fn record(&self, entry: &Value, logs: bool, usage: bool) {
        if logs {
            if let Ok(mut entries) = self.entries.lock() {
                entries.insert(0, entry.clone());
                entries.truncate(self.limit);
            }
            self.append_line(&self.logs_dir, entry);
        }
        // 用量只统计真正打到上游的请求：未命中 / 识别失败的请求没有供应商与模型可归属
        if usage && entry.get("outcome").and_then(Value::as_str) == Some("forwarded") {
            self.append_line(&self.usage_dir, &to_usage_record(entry));
        }
    }

    pub fn list(&self) -> Vec<Value> {
        self.entries.lock().map(|entries| entries.clone()).unwrap_or_default()
    }

    fn remove_day_files(&self, target: &Path) {
        for name in self.day_files(target) {
            let _ = fs::remove_file(target.join(name));
        }
    }

    // 清日志要连内存缓冲一起清：界面上那 500 条也是「日志」，不清掉会看着像没生效
    pub fn clear_logs(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
        self.remove_day_files(&self.logs_dir);
    }

    pub fn clear_usage(&self) {
        self.remove_day_files(&self.usage_dir);
    }

    fn dir_stats(&self, target: &Path) -> (i64, i64) {
        let mut bytes = 0i64;
        let mut files = 0i64;
        for name in self.day_files(target) {
            if let Ok(meta) = fs::metadata(target.join(name)) {
                bytes += meta.len() as i64;
                files += 1;
            }
        }
        (files, bytes)
    }

    pub fn stats(&self) -> Value {
        let (log_files, log_bytes) = self.dir_stats(&self.logs_dir);
        let (usage_files, usage_bytes) = self.dir_stats(&self.usage_dir);
        json!({
            "dir": self.base_dir.to_string_lossy(),
            "logs": { "files": log_files, "bytes": log_bytes },
            "usage": { "files": usage_files, "bytes": usage_bytes }
        })
    }

    // 读日期区间（含端点，YYYY-MM-DD）内的用量记录；不传就是全部
    fn read_usage_records(&self, from: Option<&str>, to: Option<&str>) -> Vec<Value> {
        let mut records = Vec::new();
        for name in self.day_files(&self.usage_dir) {
            let day = &name[..10];
            if from.map(|from| day < from).unwrap_or(false) {
                continue;
            }
            if to.map(|to| day > to).unwrap_or(false) {
                continue;
            }
            let Ok(text) = fs::read_to_string(self.usage_dir.join(name)) else {
                continue;
            };
            for line in text.split('\n') {
                if line.trim().is_empty() {
                    continue;
                }
                // 坏行跳过，不因为一行坏了丢掉整天
                if let Ok(record) = serde_json::from_str::<Value>(line) {
                    records.push(record);
                }
            }
        }
        records
    }

    // 按天 / 协议 / 供应商 / 模型几个维度一次算齐：切换维度是纯展示，不该再读一遍磁盘。
    // 模型维度不区分协议——同一个模型走哪个协议都该合在一起算
    pub fn usage(&self, options: &Value) -> Value {
        let range = resolve_range(options);
        let records = self.read_usage_records(range.from.as_deref(), range.to.as_deref());

        let mut totals = Bucket::default();
        let mut by_day: HashMap<String, Bucket> = HashMap::new();
        let mut by_protocol: HashMap<String, Bucket> = HashMap::new();
        let mut by_provider: HashMap<String, Bucket> = HashMap::new();
        let mut by_model: HashMap<String, Bucket> = HashMap::new();

        for record in &records {
            // 手工编辑或异常写入可能留下缺 timestamp 的合法 JSON 行：跳过，
            // 否则会聚出一个 "NaN-NaN-NaN" 的桶，还固定排在按天列表第一位
            let Some(timestamp) = record.get("timestamp").and_then(Value::as_i64) else {
                continue;
            };
            let day = date_key(timestamp);
            let protocol = text_or(record.get("protocol").unwrap_or(&Value::Null), "（未知协议）");
            let provider = text_or(record.get("providerName").unwrap_or(&Value::Null), "（未知供应商）");
            let model = text_or(record.get("realModel").unwrap_or(&Value::Null), "（未知模型）");

            add_to_bucket(&mut totals, record);
            for (map, key) in [
                (&mut by_day, day),
                (&mut by_protocol, protocol),
                (&mut by_provider, provider),
                (&mut by_model, model),
            ] {
                add_to_bucket(map.entry(key).or_default(), record);
            }
        }

        let with_key = |entries: Vec<(String, Bucket)>| -> Vec<Value> {
            entries
                .into_iter()
                .map(|(key, bucket)| {
                    let mut value = bucket.to_json();
                    if let Some(object) = value.as_object_mut() {
                        object.insert("key".to_string(), Value::String(key));
                    }
                    value
                })
                .collect()
        };

        // 天倒序（最近的在前），其余按输出 tokens 排——多的在上面
        let mut days: Vec<(String, Bucket)> = by_day.into_iter().collect();
        days.sort_by(|a, b| b.0.cmp(&a.0));
        let by_output = |mut entries: Vec<(String, Bucket)>| {
            entries.sort_by(|a, b| b.1.output.cmp(&a.1.output).then(b.1.requests.cmp(&a.1.requests)));
            entries
        };

        json!({
            "totals": totals.to_json(),
            "byDay": with_key(days),
            "byProtocol": with_key(by_output(by_protocol.into_iter().collect())),
            "byProvider": with_key(by_output(by_provider.into_iter().collect())),
            "byModel": with_key(by_output(by_model.into_iter().collect()))
        })
    }
}


