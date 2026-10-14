// 映射查表单测：优先级（精确 > 通配最长前缀 > 兜底）、启停跳过、错误场景

use serde_json::{json, Value};

use ccproxy_lib::proxy::router::resolve_routing;

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
                    { "id": "exact", "enabled": true, "clientModel": "claude-sonnet-4", "providerId": "p1", "realModel": "m1" },
                    { "id": "wild-short", "enabled": true, "clientModel": "claude-*", "providerId": "p1", "realModel": "w1" },
                    { "id": "wild-long", "enabled": true, "clientModel": "claude-3-5-*", "providerId": "p2", "realModel": "w2" },
                    { "id": "fallback", "enabled": true, "clientModel": "*", "providerId": "p1", "realModel": "f1" }
                ]
            },
            { "id": "openai", "name": "OpenAI", "enabled": false, "providers": [], "mappings": [] }
        ]
    });
    overrides(&mut value);
    config_from(value)
}

fn plain() -> ccproxy_lib::config::AppConfig {
    base(|_| {})
}

#[test]
fn exact_beats_wildcard_and_fallback() {
    let route = resolve_routing(&plain(), "anthropic", "claude-sonnet-4").unwrap();
    assert_eq!(route.mapping.id, "exact");
    assert_eq!(route.real_model, "m1");
    assert_eq!(route.provider_id, "p1");
}

#[test]
fn wildcard_prefers_longest_prefix() {
    // wild-long 前缀更长，但它指向的供应商 p2 已停用 → 跳过，落给 wild-short
    let route = resolve_routing(&plain(), "anthropic", "claude-3-5-haiku").unwrap();
    assert_eq!(route.mapping.id, "wild-short");
    assert_eq!(route.real_model, "w1");
}

#[test]
fn disabled_provider_makes_mapping_unusable() {
    let config = base(|value| {
        value["protocols"][0]["providers"][0]["enabled"] = json!(false);
        let mappings = value["protocols"][0]["mappings"].as_array_mut().unwrap();
        mappings.retain(|item| item["id"] != json!("fallback"));
    });
    let error = resolve_routing(&config, "anthropic", "claude-sonnet-4").unwrap_err();
    assert_eq!(error.status, 404);
}

#[test]
fn disabled_mapping_is_skipped() {
    let config = base(|value| {
        let mappings = value["protocols"][0]["mappings"].as_array_mut().unwrap();
        for item in mappings.iter_mut() {
            if item["id"] == json!("exact") {
                item["enabled"] = json!(false);
            }
        }
    });
    let route = resolve_routing(&config, "anthropic", "claude-sonnet-4").unwrap();
    assert_eq!(route.mapping.id, "wild-short");
}

#[test]
fn unmatched_falls_back() {
    let route = resolve_routing(&plain(), "anthropic", "totally-unknown").unwrap();
    assert_eq!(route.mapping.id, "fallback");
    assert_eq!(route.real_model, "f1");
}

#[test]
fn unmatched_without_fallback_is_404_with_model_and_protocol_name() {
    let config = base(|value| {
        let mappings = value["protocols"][0]["mappings"].as_array_mut().unwrap();
        mappings.retain(|item| item["id"] != json!("fallback"));
    });
    let error = resolve_routing(&config, "anthropic", "totally-unknown").unwrap_err();
    assert_eq!(error.status, 404);
    assert!(error.message.contains("totally-unknown"));
    assert!(error.message.contains("Anthropic"));
}

#[test]
fn disabled_protocol_is_503() {
    let error = resolve_routing(&plain(), "openai", "gpt-4o").unwrap_err();
    assert_eq!(error.status, 503);
}

#[test]
fn mapping_to_missing_provider_is_dropped_then_404() {
    let config = base(|value| {
        value["protocols"][0]["mappings"] = json!([
            { "id": "ghost", "enabled": true, "clientModel": "claude-sonnet-4", "providerId": "nope", "realModel": "x" }
        ]);
    });
    let error = resolve_routing(&config, "anthropic", "claude-sonnet-4").unwrap_err();
    assert_eq!(error.status, 404);
}

#[test]
fn unknown_protocol_is_404() {
    let error = resolve_routing(&plain(), "gemini", "gemini-pro").unwrap_err();
    assert_eq!(error.status, 404);
}

#[test]
fn match_kind_marks_the_winning_round() {
    // 界面的「精确/通配/兜底」徽章直接读这个字段
    assert_eq!(resolve_routing(&plain(), "anthropic", "claude-sonnet-4").unwrap().match_kind, "exact");
    assert_eq!(resolve_routing(&plain(), "anthropic", "claude-opus-9").unwrap().match_kind, "wildcard");
    assert_eq!(resolve_routing(&plain(), "anthropic", "totally-other-model").unwrap().match_kind, "fallback");
}

