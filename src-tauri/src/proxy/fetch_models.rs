// 模型组拉取：按模型组的上游格式注入认证，请求上游 GET /v1/models，解析出模型 id 列表。
// 供供应商编辑弹窗「拉取」按钮使用；走 Rust 侧请求，绕开渲染层的 CORS 限制。
// 与转发同一契约风格：绝不抛错，成败都返回 { ok, ... }。

use std::collections::BTreeSet;
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::proxy::adapters::get_adapter;
use crate::proxy::upstream::build_upstream_url;

const FETCH_TIMEOUT_MS: u64 = 10 * 1000;

// 拉取上游模型列表。入参 { apiFormat, baseUrl, token }
// apiFormat 是模型组已经明确下来的上游格式，不再接受 auto 或静默按 OpenAI 兜底。
pub async fn fetch_upstream_models(client: &reqwest::Client, payload: &Value) -> Value {
    let api_format = payload.get("apiFormat").and_then(Value::as_str).unwrap_or("");
    let base_url = payload.get("baseUrl").and_then(Value::as_str).unwrap_or("");
    let token = payload.get("token").and_then(Value::as_str).unwrap_or("");
    let protocol_id = match api_format {
        "anthropic" => "anthropic",
        "openai" | "openai_chat" | "openai_responses" => "openai_chat",
        _ => return json!({ "ok": false, "error": "模型组的上游格式无效，请先选择正确的协议" }),
    };

    let mut headers = HeaderMap::new();
    headers.insert("accept", axum::http::HeaderValue::from_static("application/json"));
    match get_adapter(protocol_id) {
        Ok(adapter) => {
            if !token.is_empty() {
                adapter.apply_auth(&mut headers, token);
            }
            if protocol_id == "anthropic" {
                headers.insert("anthropic-version", axum::http::HeaderValue::from_static("2023-06-01"));
            }
        }
        Err(error) => return json!({ "ok": false, "error": error }),
    }

    let url = build_upstream_url(base_url, "/v1/models");
    let request = client.get(&url).headers(headers);
    let response = match tokio::time::timeout(Duration::from_millis(FETCH_TIMEOUT_MS), request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let message = if error.is_connect() {
                format!("上游地址无效：{error}")
            } else {
                error.to_string()
            };
            return json!({ "ok": false, "error": message });
        }
        Err(_) => {
            return json!({
                "ok": false,
                "error": format!("拉取超时（{} 秒）", FETCH_TIMEOUT_MS / 1000)
            })
        }
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return json!({ "ok": false, "error": format!("上游返回 HTTP {status}") });
    }

    let Ok(payload) = response.json::<Value>().await else {
        return json!({ "ok": false, "error": "上游响应里没有可识别的模型列表" });
    };
    // 两种协议的模型列表响应结构一致：顶层 data 数组、条目带 id
    let Some(items) = payload.get("data").and_then(Value::as_array) else {
        return json!({ "ok": false, "error": "上游响应里没有可识别的模型列表" });
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut models: Vec<String> = Vec::new();
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("").trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        models.push(id.to_string());
    }
    json!({ "ok": true, "models": models })
}
