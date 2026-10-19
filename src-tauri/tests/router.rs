// 映射查表单测：列表顺序即优先级（首条命中）、启停跳过、错误场景

use serde_json::{json, Value};

use ccproxy_lib::proxy::router::resolve_routing;

use common::config_from;

mod common;

fn base(overrides: impl FnOnce(&mut Value)) -> ccproxy_lib::config::AppConfig {
    let mut value = json!({
        // v5 起列表顺序即优先级；声明版本避免加载时按旧语义重排
        "version": 5,
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
            { "id": "openai_chat", "name": "OpenAI Chat", "enabled": false, "providers": [], "mappings": [] }
        ]
    });
    overrides(&mut value);
    config_from(value)
}

fn plain() -> ccproxy_lib::config::AppConfig {
    base(|_| {})
}

#[test]
fn first_matching_mapping_wins() {
    // fixture 里精确规则排在最前，因此先于通配和兜底命中
    let route = resolve_routing(&plain(), "anthropic", "claude-sonnet-4").unwrap();
    assert_eq!(route.mapping.id, "exact");
    assert_eq!(route.real_model, "m1");
    assert_eq!(route.provider_id, "p1");
    // 这条供应商是从老配置的 anthropic 协议下提升上来的，格式按归属协议推断成 anthropic
    assert_eq!(route.api_format, "anthropic");
}

#[test]
fn openai_group_uses_chat_for_anthropic_clients() {
    // Anthropic 客户端映射到 OpenAI 模型组时，默认走 Chat 转换。
    let config = base(|value| {
        value["protocols"][0]["providers"][0]["apiFormat"] = json!("openai_responses");
    });
    let route = resolve_routing(&config, "anthropic", "claude-sonnet-4").unwrap();
    assert_eq!(route.api_format, "openai_chat");
    assert_eq!(route.model_group_id, "g_openai");
}

#[test]
fn openai_group_uses_responses_for_responses_clients() {
    let config = config_from(json!({
        "providers": [{
            "id": "p1",
            "name": "OpenAI",
            "enabled": true,
            "baseUrl": "http://upstream.test",
            "token": "t",
            "modelGroups": [{
                "id": "g_openai",
                "apiFormat": "openai",
                "models": [{ "name": "gpt-5.5" }]
            }]
        }],
        "protocols": [{
            "id": "openai_responses",
            "enabled": true,
            "mappings": [{
                "id": "m1",
                "enabled": true,
                "clientModel": "gpt-5.5",
                "providerId": "p1",
                "modelGroupId": "g_openai",
                "realModel": "gpt-5.5"
            }]
        }]
    }));
    let route = resolve_routing(&config, "openai_responses", "gpt-5.5").unwrap();
    assert_eq!(route.api_format, "openai_responses");
}

#[test]
fn mapping_upstream_protocol_overrides_client_protocol() {
    let config = config_from(json!({
        "providers": [{
            "id": "p1",
            "name": "OpenAI",
            "enabled": true,
            "baseUrl": "http://upstream.test",
            "token": "t",
            "modelGroups": [{
                "id": "g_openai",
                "apiFormat": "openai",
                "models": [{ "name": "gpt-5.5" }]
            }]
        }],
        "protocols": [{
            "id": "anthropic",
            "enabled": true,
            "mappings": [{
                "id": "m1",
                "enabled": true,
                "clientModel": "claude-*",
                "providerId": "p1",
                "modelGroupId": "g_openai",
                "upstreamProtocol": "openai_responses",
                "realModel": "gpt-5.5"
            }]
        }]
    }));
    let route = resolve_routing(&config, "anthropic", "claude-sonnet-4").unwrap();
    assert_eq!(route.api_format, "openai_responses");
}

#[test]
fn earlier_short_wildcard_beats_later_long_wildcard() {
    // wild-long 前缀更长，但 wild-short 排在前面：首条命中即生效，不再按前缀长度排序
    let config = base(|value| {
        value["protocols"][0]["providers"][1]["enabled"] = json!(true);
    });
    let route = resolve_routing(&config, "anthropic", "claude-3-5-haiku").unwrap();
    assert_eq!(route.mapping.id, "wild-short");
    assert_eq!(route.real_model, "w1");
}

#[test]
fn moving_a_mapping_up_gives_it_priority() {
    let config = base(|value| {
        value["protocols"][0]["mappings"] = json!([
            { "id": "wild-first", "enabled": true, "clientModel": "claude-*", "providerId": "p1", "realModel": "w1" },
            { "id": "exact-second", "enabled": true, "clientModel": "claude-3-5-haiku", "providerId": "p1", "realModel": "m1" }
        ]);
    });
    let route = resolve_routing(&config, "anthropic", "claude-3-5-haiku").unwrap();
    assert_eq!(route.mapping.id, "wild-first");
    assert_eq!(route.match_kind, "wildcard");
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
    let error = resolve_routing(&plain(), "openai_chat", "gpt-4o").unwrap_err();
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
fn match_kind_reflects_winning_pattern() {
    // 界面的「精确/通配/兜底」徽章直接读这个字段
    assert_eq!(
        resolve_routing(&plain(), "anthropic", "claude-sonnet-4")
            .unwrap()
            .match_kind,
        "exact"
    );
    assert_eq!(
        resolve_routing(&plain(), "anthropic", "claude-opus-9")
            .unwrap()
            .match_kind,
        "wildcard"
    );
    assert_eq!(
        resolve_routing(&plain(), "anthropic", "totally-other-model")
            .unwrap()
            .match_kind,
        "fallback"
    );
}
