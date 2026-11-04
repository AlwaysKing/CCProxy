// 模型列表接口单测：路径匹配、协议判定、可用映射过滤、两种协议的响应格式

use axum::http::HeaderMap;
use serde_json::{json, Value};

use ccproxy_lib::proxy::models::{build_models_response, match_models_request, resolve_client_protocol, usable_client_models};

use common::config_from;

mod common;

fn base(overrides: impl FnOnce(&mut Value)) -> ccproxy_lib::config::AppConfig {
    let mut value = json!({
        "protocols": [
            {
                "id": "anthropic", "name": "Anthropic", "enabled": true,
                "providers": [
                    { "id": "p1", "name": "供应商一", "enabled": true, "baseUrl": "http://upstream-a.test", "token": "t1", "models": ["m1"] },
                    { "id": "p2", "name": "供应商二", "enabled": false, "baseUrl": "http://upstream-b.test", "token": "t2", "models": ["m2"] }
                ],
                "mappings": [
                    { "id": "m-a", "enabled": true, "clientModel": "claude-opus-4", "providerId": "p1", "realModel": "glm-4.6" },
                    { "id": "m-b", "enabled": true, "clientModel": "claude-sonnet-4", "providerId": "p1", "realModel": "glm-4.5" },
                    { "id": "m-off", "enabled": false, "clientModel": "claude-haiku-4", "providerId": "p1", "realModel": "x" },
                    { "id": "m-dead-provider", "enabled": true, "clientModel": "claude-3", "providerId": "p2", "realModel": "x" },
                    { "id": "m-dup", "enabled": true, "clientModel": "claude-opus-4", "providerId": "p1", "realModel": "x" },
                    { "id": "m-wild", "enabled": true, "clientModel": "claude-sonnet-4-*", "providerId": "p1", "realModel": "x" },
                    { "id": "m-fallback", "enabled": true, "clientModel": "*", "providerId": "p1", "realModel": "f1" }
                ]
            },
            { "id": "openai", "name": "OpenAI", "enabled": false, "providers": [], "mappings": [] }
        ]
    });
    overrides(&mut value);
    config_from(value)
}

#[test]
fn match_models_request_only_accepts_get_models_paths() {
    assert_eq!(match_models_request("GET", "/v1/models").unwrap().model_id, None);
    assert_eq!(match_models_request("GET", "/v1/models?limit=20").unwrap().model_id, None);
    assert_eq!(
        match_models_request("GET", "/v1/models/claude-opus-4").unwrap().model_id.as_deref(),
        Some("claude-opus-4")
    );
    assert_eq!(
        match_models_request("GET", "/v1/models/%E6%A8%A1%E5%9E%8B").unwrap().model_id.as_deref(),
        Some("模型")
    );
    // 非法百分号编码不抛异常，按原始字符串处理（后续走未命中 → 404）
    assert_eq!(
        match_models_request("GET", "/v1/models/%E4%B8").unwrap().model_id.as_deref(),
        Some("%E4%B8")
    );
    assert!(match_models_request("POST", "/v1/models").is_none());
    assert!(match_models_request("GET", "/v1/messages").is_none());
    assert!(match_models_request("GET", "/v1/models/a/b").is_none());
}

#[test]
fn resolve_client_protocol_prefers_anthropic_headers() {
    let mut map = HeaderMap::new();
    map.insert("anthropic-version", "2023-06-01".parse().unwrap());
    assert_eq!(resolve_client_protocol(&map, "/v1/models"), "anthropic");

    let mut map = HeaderMap::new();
    map.insert("x-api-key", "sk-fake".parse().unwrap());
    assert_eq!(resolve_client_protocol(&map, "/v1/models"), "anthropic");

    let mut map = HeaderMap::new();
    map.insert("authorization", "Bearer sk-fake".parse().unwrap());
    assert_eq!(resolve_client_protocol(&map, "/v1/models"), "openai_chat");
    assert_eq!(resolve_client_protocol(&HeaderMap::new(), "/v1/models"), "openai_chat");
}

#[test]
fn resolve_client_protocol_uses_model_discovery_query_parameters() {
    let mut anthropic_headers = HeaderMap::new();
    anthropic_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

    assert_eq!(
        resolve_client_protocol(&HeaderMap::new(), "/v1/models?client_version=0.165.0"),
        "codex"
    );
    assert_eq!(
        resolve_client_protocol(&anthropic_headers, "/v1/models?client_version=0.165.0&limit=1000"),
        "codex"
    );
    assert_eq!(
        resolve_client_protocol(&HeaderMap::new(), "/v1/models?limit=1000"),
        "anthropic"
    );
    assert_eq!(
        resolve_client_protocol(&HeaderMap::new(), "/v1/models?client_version="),
        "openai_chat"
    );
}

#[test]
fn usable_client_models_skips_disabled_and_wildcards() {
    let config = base(|_| {});
    let names = usable_client_models(config.protocol("anthropic").unwrap(), &config.providers);
    assert_eq!(names, vec!["claude-opus-4".to_string(), "claude-sonnet-4".to_string()]);
}

#[test]
fn anthropic_list_format() {
    let config = base(|_| {});
    let (status, payload) = build_models_response(&config, "anthropic", None);
    assert_eq!(status, 200);
    let data = payload["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["id"], json!("claude-opus-4"));
    assert_eq!(data[0]["type"], json!("model"));
    assert_eq!(data[0]["display_name"], json!("claude-opus-4"));
    assert!(data[0]["created_at"].is_string());
    assert_eq!(payload["first_id"], json!("claude-opus-4"));
    assert_eq!(payload["last_id"], json!("claude-sonnet-4"));
    assert_eq!(payload["has_more"], json!(false));
}

#[test]
fn openai_list_format() {
    let config = base(|value| {
        value["protocols"][1]["enabled"] = json!(true);
        value["protocols"][1]["providers"] =
            json!([{ "id": "q1", "name": "供应商", "enabled": true, "baseUrl": "http://upstream-c.test", "token": "t", "models": ["m"] }]);
        value["protocols"][1]["mappings"] =
            json!([{ "id": "m-o1", "enabled": true, "clientModel": "gpt-4o", "providerId": "q1", "realModel": "deepseek-chat" }]);
    });
    let (status, payload) = build_models_response(&config, "openai_chat", None);
    assert_eq!(status, 200);
    assert_eq!(payload["object"], json!("list"));
    let data = payload["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], json!("gpt-4o"));
    assert_eq!(data[0]["object"], json!("model"));
    assert_eq!(data[0]["owned_by"], json!("ccproxy"));
    assert!(data[0]["created"].is_i64());
}

#[test]
fn codex_catalog_format_uses_openai_responses_mappings() {
    let config = base(|value| {
        value["protocols"][1]["enabled"] = json!(true);
        value["protocols"][1]["providers"] = json!([{
            "id": "p1",
            "name": "Codex 供应商",
            "enabled": true,
            "baseUrl": "http://codex.example.com",
            "token": "t",
            "models": [{
                "name": "m1",
                "contextWindow": 128000,
                "maxContextWindow": 256000,
                "inputModalities": ["text", "image"],
                "reasoning": true
            }]
        }]);
        value["protocols"][1]["mappings"] = json!([
            { "id": "m-codex", "enabled": true, "clientModel": "codex-model", "providerId": "p1", "realModel": "m1" },
            { "id": "m-codex-wild", "enabled": true, "clientModel": "codex-*", "providerId": "p1", "realModel": "m1" }
        ]);
    });

    let (status, payload) = build_models_response(&config, "codex", None);

    assert_eq!(status, 200);
    assert!(payload.get("data").is_none());
    let models = payload["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["slug"], json!("codex-model"));
    assert_eq!(models[0]["display_name"], json!("codex-model"));
    assert_eq!(models[0]["context_window"], json!(128000));
    assert_eq!(models[0]["max_context_window"], json!(256000));
    assert_eq!(models[0]["input_modalities"], json!(["text", "image"]));
    assert!(!models[0]["supported_reasoning_levels"].as_array().unwrap().is_empty());
    assert_eq!(models[0]["visibility"], json!("list"));
    assert_eq!(models[0]["supported_in_api"], json!(true));
    assert!(models[0].get("truncation_policy").is_some());
    assert!(models[0].get("base_instructions").is_some());
}

#[test]
fn single_model_hit_returns_entry() {
    let config = base(|_| {});
    let (status, payload) = build_models_response(&config, "anthropic", Some("claude-opus-4"));
    assert_eq!(status, 200);
    assert_eq!(payload["id"], json!("claude-opus-4"));
    assert_eq!(payload["type"], json!("model"));
}

#[test]
fn single_model_miss_returns_protocol_specific_404() {
    let config = base(|_| {});
    let (status, payload) = build_models_response(&config, "anthropic", Some("no-such-model"));
    assert_eq!(status, 404);
    assert_eq!(payload["error"]["type"], json!("not_found_error"));

    let openai = base(|value| value["protocols"][1]["enabled"] = json!(true));
    let (status, payload) = build_models_response(&openai, "openai_chat", Some("no-such-model"));
    assert_eq!(status, 404);
    assert_eq!(payload["error"]["code"], json!("model_not_found"));
}

#[test]
fn disabled_and_missing_protocol() {
    let config = base(|_| {});
    let (status, payload) = build_models_response(&config, "openai_chat", None);
    assert_eq!(status, 503);
    assert_eq!(payload["error"]["type"], json!("protocol_disabled"));

    let (status, payload) = build_models_response(&config, "nope", None);
    assert_eq!(status, 404);
    assert_eq!(payload["error"]["type"], json!("protocol_not_found"));
}

