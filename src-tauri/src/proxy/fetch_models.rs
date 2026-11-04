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
const CODEX_DISCOVERY_PATH: &str = concat!(
    "/v1/models?client_version=",
    env!("CARGO_PKG_VERSION")
);

async fn request_models(client: &reqwest::Client, url: &str, headers: HeaderMap) -> Result<Value, String> {
    let request = client.get(url).headers(headers);
    let response = match tokio::time::timeout(Duration::from_millis(FETCH_TIMEOUT_MS), request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let message = if error.is_connect() {
                format!("上游地址无效：{error}")
            } else {
                error.to_string()
            };
            return Err(message);
        }
        Err(_) => return Err(format!("拉取超时（{} 秒）", FETCH_TIMEOUT_MS / 1000)),
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("上游返回 HTTP {status}"));
    }

    response
        .json::<Value>()
        .await
        .map_err(|_| "上游响应里没有可识别的模型列表".to_string())
}

// data[].id 是 OpenAI Chat 与 Claude 的标准模型列表；Codex 原生目录用 models[].slug。
fn extract_models(payload: &Value, key: &str, id_key: &str) -> Option<Vec<String>> {
    let items = payload.get(key)?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|item| item.get(id_key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn push_models(models: &mut Vec<String>, seen: &mut BTreeSet<String>, page: Vec<String>) {
    for model in page {
        if seen.insert(model.clone()) {
            models.push(model);
        }
    }
}

// 拉取上游模型列表。入参 { apiFormat, baseUrl, token }
// apiFormat 是模型组已经明确下来的上游格式；OpenAI Chat 与 Responses 各走各的发现协议。
pub async fn fetch_upstream_models(client: &reqwest::Client, payload: &Value) -> Value {
    let api_format = payload.get("apiFormat").and_then(Value::as_str).unwrap_or("");
    let base_url = payload.get("baseUrl").and_then(Value::as_str).unwrap_or("");
    let token = payload.get("token").and_then(Value::as_str).unwrap_or("");
    let protocol_id = match api_format {
        "anthropic" => "anthropic",
        "openai_chat" | "openai_responses" => "openai_chat",
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

    let mut models: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    if api_format == "anthropic" {
        // Claude Code 的模型发现请求固定带 limit=1000；省略时网关通常按 OpenAI
        // 模型目录处理，会拿到错误协议分支的列表。
        let url = build_upstream_url(base_url, "/v1/models?limit=1000");
        let payload = match request_models(client, &url, headers).await {
            Ok(payload) => payload,
            Err(error) => return json!({ "ok": false, "error": error }),
        };
        let Some(page) = extract_models(&payload, "data", "id") else {
            return json!({ "ok": false, "error": "上游响应里没有可识别的模型列表" });
        };
        push_models(&mut models, &mut seen, page);
    } else if api_format == "openai_chat" {
        let url = build_upstream_url(base_url, "/v1/models");
        let payload = match request_models(client, &url, headers).await {
            Ok(payload) => payload,
            Err(error) => return json!({ "ok": false, "error": error }),
        };
        let Some(page) = extract_models(&payload, "data", "id") else {
            return json!({ "ok": false, "error": "OpenAI Chat 响应里没有 data[].id" });
        };
        push_models(&mut models, &mut seen, page);
    } else {
        let url = build_upstream_url(base_url, CODEX_DISCOVERY_PATH);
        let payload = match request_models(client, &url, headers).await {
            Ok(payload) => payload,
            Err(error) => return json!({ "ok": false, "error": error }),
        };
        let Some(page) = extract_models(&payload, "models", "slug") else {
            return json!({ "ok": false, "error": "OpenAI Responses / Codex 响应里没有 models[].slug" });
        };
        push_models(&mut models, &mut seen, page);
    }

    json!({ "ok": true, "models": models })
}
