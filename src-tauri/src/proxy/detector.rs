// 协议识别器：根据请求路径、请求头、请求体特征判断请求属于哪种 agent 协议。
// 打分制：anthropic 与 openai 各自累计得分，高分者胜；并列或全零视为无法识别。

use axum::http::HeaderMap;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default)]
pub struct Scores {
    pub anthropic: i64,
    pub openai: i64,
}

pub struct Detection {
    pub protocol: Option<String>,
    pub scores: Scores,
    pub reasons: Vec<String>,
}

// 路径规则：命中即给该协议加分。分数刻意给得比头/体特征高——路径是最强信号
fn match_path(pathname: &str) -> Vec<(&'static str, i64)> {
    let mut hits = Vec::new();
    if let Some(rest) = pathname.strip_prefix("/v1/messages") {
        if rest.is_empty() || rest.starts_with('/') {
            hits.push(("anthropic", 3));
        }
    }
    for suffix in ["chat/completions", "responses", "completions", "embeddings", "models"] {
        let prefix = format!("/v1/{suffix}");
        if let Some(rest) = pathname.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with('/') {
                hits.push(("openai", 3));
                break;
            }
        }
    }
    hits
}

// 解析请求体为 JSON 对象；失败或非对象返回 None，绝不抛异常
pub fn parse_body(body: &[u8]) -> Option<Value> {
    if body.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_slice(body).ok()?;
    if parsed.is_object() {
        Some(parsed)
    } else {
        None
    }
}

fn header_present(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .map(|value| !value.as_bytes().is_empty())
        .unwrap_or(false)
}

// 识别入口
pub fn detect_protocol(path: &str, headers: &HeaderMap, body: &[u8]) -> Detection {
    let mut scores = Scores::default();
    let mut reasons: Vec<String> = Vec::new();
    let pathname = path.split('?').next().unwrap_or("");

    for (protocol, score) in match_path(pathname) {
        if protocol == "anthropic" {
            scores.anthropic += score;
        } else {
            scores.openai += score;
        }
        reasons.push(format!("路径 {pathname} 命中 {protocol} 路径规则 +{score}"));
    }
    if header_present(headers, "anthropic-version") {
        scores.anthropic += 3;
        reasons.push("请求头 anthropic-version +3".to_string());
    }
    if header_present(headers, "x-api-key") {
        scores.anthropic += 1;
        reasons.push("请求头 x-api-key +1".to_string());
    }
    if let Some(payload) = parse_body(body) {
        if payload.get("system").is_some() && payload.get("max_tokens").is_some() {
            scores.anthropic += 2;
            reasons.push("请求体顶层同时含 system 与 max_tokens +2".to_string());
        }
    }

    let protocol = if scores.anthropic == scores.openai {
        None
    } else if scores.anthropic > scores.openai {
        Some("anthropic".to_string())
    } else {
        Some("openai".to_string())
    };

    Detection { protocol, scores, reasons }
}

