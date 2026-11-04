// 模型列表接口接管：GET /v1/models 与 GET /v1/models/{id} 不透传上游，
// 由代理按当前激活的映射表直接生成响应，客户端看到的就是本代理实际可路由的模型名。
// OpenAI Chat、Claude Code 与 Codex 共用路径，按官方客户端参数和请求头分流：
// client_version -> Codex，limit=1000 或 Anthropic 请求头 -> Claude，其余 -> OpenAI Chat。

use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::clients::catalog::{build_catalog_entry, expected_catalog_entries};
use crate::config::{AppConfig, Protocol, Provider};

// 与 router 的可用性口径一致：映射启用、供应商存在且启用才算可路由。
// 供应商来自配置顶层的全局池
pub fn usable_client_models(protocol: &Protocol, providers: &[Provider]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for mapping in &protocol.mappings {
        if !mapping.enabled {
            continue;
        }
        // 含 * 的通配（含兜底 *）不是真实模型名，不列入列表（请求时仍正常路由）
        if mapping.client_model.is_empty() || mapping.client_model.contains('*') {
            continue;
        }
        let Some(provider) = providers.iter().find(|item| item.id == mapping.provider_id) else {
            continue;
        };
        if !provider.enabled {
            continue;
        }
        if !names.iter().any(|name| name == &mapping.client_model) {
            names.push(mapping.client_model.clone());
        }
    }
    names
}

pub struct ModelsRequest {
    pub model_id: Option<String>,
}

// decodeURIComponent 的等价物：编码不合法（半个 %、非法十六进制、非法 UTF-8 序列）
// 时退回原串，与浏览器抛 URIError 后调用方按未命中处理同一效果
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return raw.to_string();
            }
            let Some(value) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
            else {
                return raw.to_string();
            };
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_string())
}

// 是否为代理接管的模型列表请求
pub fn match_models_request(method: &str, path: &str) -> Option<ModelsRequest> {
    if method != "GET" {
        return None;
    }
    let pathname = path.split('?').next().unwrap_or("");
    let rest = pathname.strip_prefix("/v1/models")?;
    if rest.is_empty() {
        return Some(ModelsRequest { model_id: None });
    }
    let id = rest.strip_prefix('/')?;
    // 只允许 /v1/models/<一段>
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(ModelsRequest { model_id: Some(percent_decode(id)) })
}

// 按官方模型发现参数判定客户端类型。查询参数优先于请求头，避免 Claude Code
// 同时携带 Anthropic 请求头和 client_version 时被误判。
pub fn resolve_client_protocol(headers: &HeaderMap, path: &str) -> &'static str {
    let mut has_client_version = false;
    let mut has_claude_limit = false;
    if let Some((_, query)) = path.split_once('?') {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if key == "client_version" && !value.trim().is_empty() {
                has_client_version = true;
            }
            if key == "limit" && value == "1000" {
                has_claude_limit = true;
            }
        }
    }
    if has_client_version {
        return "codex";
    }
    if has_claude_limit {
        return "anthropic";
    }
    if headers.contains_key("anthropic-version") || headers.contains_key("x-api-key") {
        "anthropic"
    } else {
        "openai_chat"
    }
}

fn now_iso() -> String {
    crate::now_iso()
}

// 组装对应协议的模型对象
fn build_model_entry(protocol_id: &str, id: &str) -> Value {
    if protocol_id == "anthropic" {
        json!({ "type": "model", "id": id, "display_name": id, "created_at": now_iso() })
    } else {
        json!({ "id": id, "object": "model", "created": chrono::Utc::now().timestamp(), "owned_by": "ccproxy" })
    }
}

// 组装对应协议的 404 响应体
fn build_not_found_payload(protocol_id: &str, model_id: &str) -> Value {
    if protocol_id == "anthropic" {
        json!({ "type": "error", "error": { "type": "not_found_error", "message": format!("model: {model_id} not found") } })
    } else {
        json!({ "error": { "message": format!("The model '{model_id}' does not exist"), "type": "invalid_request_error", "code": "model_not_found" } })
    }
}

// 生成响应入口。返回 (status, payload)，由调用方序列化写出
pub fn build_models_response(config: &AppConfig, protocol_id: &str, model_id: Option<&str>) -> (u16, Value) {
    let mapping_protocol_id = if protocol_id == "codex" { "openai_responses" } else { protocol_id };
    let Some(protocol) = config.protocol(mapping_protocol_id) else {
        return (
            404,
            json!({ "error": { "type": "protocol_not_found", "message": format!("配置中不存在协议 {mapping_protocol_id}") } }),
        );
    };
    if !protocol.enabled {
        return (
            503,
            json!({ "error": { "type": "protocol_disabled", "message": format!("协议 {}（{}）已停用", protocol.name, protocol.id) } }),
        );
    }

    let names = usable_client_models(protocol, &config.providers);

    if protocol_id == "codex" {
        let entries = expected_catalog_entries(Some(protocol), &config.providers);
        if let Some(model_id) = model_id {
            let Some(entry) = entries.iter().find(|entry| entry.name == model_id) else {
                return (404, build_not_found_payload(protocol_id, model_id));
            };
            return (200, build_catalog_entry(entry));
        }
        let models: Vec<Value> = entries.iter().map(build_catalog_entry).collect();
        return (200, json!({ "models": models }));
    }

    // 单模型查询：列表里有就返回单项，没有返回协议各自的 404 形态
    if let Some(model_id) = model_id {
        if !names.iter().any(|name| name == model_id) {
            return (404, build_not_found_payload(protocol_id, model_id));
        }
        return (200, build_model_entry(protocol_id, model_id));
    }

    let data: Vec<Value> = names.iter().map(|name| build_model_entry(protocol_id, name)).collect();
    if protocol_id == "anthropic" {
        let first = names.first().cloned();
        let last = names.last().cloned();
        return (
            200,
            json!({ "data": data, "first_id": first, "last_id": last, "has_more": false, "next_page": Value::Null }),
        );
    }
    (200, json!({ "object": "list", "data": data }))
}

