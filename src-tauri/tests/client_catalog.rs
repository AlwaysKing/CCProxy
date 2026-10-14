// codex 模型清单单测：期望条目的推导、清单检查、定点改写

use serde_json::{json, Map, Value};

use ccproxy_lib::clients::catalog::{expected_catalog_entries, inspect_catalog, rewrite_catalog, ExpectedEntry};

use common::config_from;

mod common;

// 一条合格的清单条目：字段齐全，能力值由调用方按需覆盖
fn make_entry(slug: &str, overrides: Value) -> Value {
    let mut entry = json!({
        "slug": slug,
        "display_name": slug,
        "description": slug,
        "base_instructions": "You are Codex.",
        "context_window": 1024000,
        "max_context_window": 1024000,
        "input_modalities": ["text", "image"],
        "default_reasoning_level": "high",
        "supported_reasoning_levels": [
            { "effort": "none", "description": "Disable thinking" },
            { "effort": "low", "description": "Fast responses with lighter reasoning" },
            { "effort": "high", "description": "Enabled high reasoning" },
            { "effort": "max", "description": "Maximum reasoning depth for the hardest problems" }
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1000,
        "support_verbosity": false,
        "truncation_policy": { "mode": "bytes", "limit": 10000 },
        "experimental_supported_tools": []
    });
    if let (Some(target), Some(source)) = (entry.as_object_mut(), overrides.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    entry
}

fn want(name: &str, overrides: Value) -> ExpectedEntry {
    let mut entry = ExpectedEntry {
        name: name.to_string(),
        context_window: Some(1024000),
        max_context_window: Some(1024000),
        input_modalities: Some(vec!["text".to_string(), "image".to_string()]),
        reasoning: true,
    };
    if let Some(source) = overrides.as_object() {
        for (key, value) in source {
            match key.as_str() {
                "contextWindow" => entry.context_window = value.as_u64(),
                "maxContextWindow" => entry.max_context_window = value.as_u64(),
                "inputModalities" => {
                    entry.input_modalities = value
                        .as_array()
                        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
                }
                "reasoning" => entry.reasoning = value.as_bool().unwrap_or(false),
                _ => {}
            }
        }
    }
    entry
}

fn catalog_text(entries: Vec<Value>) -> String {
    format!("{}\n", serde_json::to_string_pretty(&json!({ "models": entries })).unwrap())
}

// ============================================================================ 期望条目推导

#[test]
fn expected_entries_use_client_model_name_and_real_model_capabilities() {
    let config = config_from(json!({
        "protocols": [{
            "id": "openai", "name": "OpenAI", "enabled": true,
            "providers": [
                { "id": "p1", "enabled": true, "baseUrl": "http://a", "models": [
                    { "name": "real-a", "contextWindow": 1024000, "maxContextWindow": 2048000, "inputModalities": ["text", "image"], "reasoning": true },
                    { "name": "real-b" }
                ] },
                { "id": "p2", "enabled": false, "baseUrl": "http://b", "models": [{ "name": "x" }] }
            ],
            "mappings": [
                { "id": "m1", "clientModel": "client-a", "providerId": "p1", "realModel": "real-a", "enabled": true },
                { "id": "m2", "clientModel": "client-b", "providerId": "p1", "realModel": "real-b", "enabled": false },
                { "id": "m3", "clientModel": "*", "providerId": "p1", "realModel": "real-a", "enabled": true },
                { "id": "m4", "clientModel": "client-x", "providerId": "p2", "realModel": "x", "enabled": true }
            ]
        }]
    }));
    let entries = expected_catalog_entries(config.protocol("openai"));
    // 停用的映射、通配/兜底映射、停用供应商的映射都不算模型
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "client-a");
    assert_eq!(entries[0].context_window, Some(1024000));
    assert_eq!(entries[0].max_context_window, Some(2048000));
    assert_eq!(entries[0].input_modalities, Some(vec!["text".to_string(), "image".to_string()]));
    assert!(entries[0].reasoning);
}

#[test]
fn unconfigured_capabilities_stay_none() {
    let config = config_from(json!({
        "protocols": [{
            "id": "openai", "name": "OpenAI", "enabled": true,
            "providers": [{ "id": "p1", "enabled": true, "baseUrl": "http://a", "models": [{ "name": "real-a" }] }],
            "mappings": [{ "id": "m1", "clientModel": "client-a", "providerId": "p1", "realModel": "real-a", "enabled": true }]
        }]
    }));
    let entries = expected_catalog_entries(config.protocol("openai"));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].context_window, None);
    assert_eq!(entries[0].max_context_window, None);
    assert_eq!(entries[0].input_modalities, None);
    assert!(!entries[0].reasoning);
}

#[test]
fn duplicate_mappings_keep_only_the_first_and_disabled_protocol_yields_nothing() {
    let config = config_from(json!({
        "protocols": [{
            "id": "openai", "name": "OpenAI", "enabled": true,
            "providers": [{ "id": "p1", "enabled": true, "baseUrl": "http://a", "models": [
                { "name": "real-a", "contextWindow": 111 }, { "name": "real-b", "contextWindow": 222 }
            ] }],
            "mappings": [
                { "id": "m1", "clientModel": "client-a", "providerId": "p1", "realModel": "real-a", "enabled": true },
                { "id": "m2", "clientModel": "client-a", "providerId": "p1", "realModel": "real-b", "enabled": true }
            ]
        }]
    }));
    let entries = expected_catalog_entries(config.protocol("openai"));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].context_window, Some(111));

    let disabled = config_from(json!({
        "protocols": [{ "id": "openai", "name": "OpenAI", "enabled": false, "providers": [], "mappings": [] }]
    }));
    assert!(expected_catalog_entries(disabled.protocol("openai")).is_empty());
}

// ============================================================================ 检查

#[test]
fn inspect_ok_when_entry_matches() {
    let result = inspect_catalog(&catalog_text(vec![make_entry("client-a", json!({}))]), &[want("client-a", json!({}))]);
    assert_eq!(result.status, "ok");
    assert!(result.problems.is_empty());
}

#[test]
fn inspect_reports_missing_and_mismatched_capabilities() {
    let result = inspect_catalog(
        &catalog_text(vec![
            make_entry(
                "client-a",
                json!({ "context_window": 999, "max_context_window": 999, "input_modalities": ["text"], "supported_reasoning_levels": [] }),
            ),
            make_entry("other-provider", json!({ "context_window": 1 })),
        ]),
        &[want("client-a", json!({})), want("client-b", json!({}))],
    );
    assert_eq!(result.status, "mismatch");
    for needle in [
        "没有 client-b",
        "上下文窗口是 999",
        "最大上下文窗口是 999",
        "输入模态是 text",
        "没有推理档位",
    ] {
        assert!(
            result.problems.iter().any(|item| item.contains(needle)),
            "缺少提示：{needle}（实际：{:?}）",
            result.problems
        );
    }
    // 别的供应商的条目不比能力
    assert!(!result.problems.iter().any(|item| item.contains("other-provider")));
}

#[test]
fn inspect_reports_reasoning_levels_when_ccproxy_has_none() {
    let result = inspect_catalog(
        &catalog_text(vec![make_entry("client-a", json!({}))]),
        &[want("client-a", json!({ "reasoning": false }))],
    );
    assert_eq!(result.status, "mismatch");
    assert!(result.problems.iter().any(|item| item.contains("配了推理档位")));
}

#[test]
fn inspect_ignores_capabilities_ccproxy_did_not_configure() {
    let result = inspect_catalog(
        &catalog_text(vec![make_entry("client-a", json!({}))]),
        &[want(
            "client-a",
            json!({ "contextWindow": null, "maxContextWindow": null, "inputModalities": null }),
        )],
    );
    assert_eq!(result.status, "ok");
}

#[test]
fn inspect_reports_broken_json_duplicates_and_missing_required_keys() {
    assert_eq!(inspect_catalog("{ 坏的", &[want("client-a", json!({}))]).status, "unreadable");

    let duplicated = inspect_catalog(
        &catalog_text(vec![make_entry("client-a", json!({})), make_entry("client-a", json!({}))]),
        &[want("client-a", json!({}))],
    );
    assert!(duplicated.problems.iter().any(|item| item.contains("出现了 2 次")));

    let broken = inspect_catalog(
        &catalog_text(vec![json!({ "slug": "client-a", "display_name": "x" })]),
        &[want("client-a", json!({}))],
    );
    assert!(broken.problems.iter().any(|item| item.contains("缺必需字段")));
    assert!(broken.problems.iter().any(|item| item.contains("缺 base_instructions")));
}

#[test]
fn foreign_entries_missing_keys_are_reported_as_not_ours() {
    let result = inspect_catalog(
        &catalog_text(vec![make_entry("client-a", json!({})), json!({ "slug": "foreign", "display_name": "x" })]),
        &[want("client-a", json!({}))],
    );
    assert_eq!(result.status, "mismatch");
    assert!(result
        .problems
        .iter()
        .any(|item| item.contains("foreign") && item.contains("不归 CCProxy 管")));
}

// ============================================================================ 改写

#[test]
fn rewrite_changes_only_our_fields() {
    let before = catalog_text(vec![make_entry("client-a", json!({ "context_window": 999 })), make_entry("foreign", json!({}))]);
    let result = rewrite_catalog(&before, &[want("client-a", json!({}))]).unwrap();
    assert!(result.changed);
    let after: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(after["models"][0]["context_window"], json!(1024000));
    // 别的条目原样
    assert_eq!(after["models"][1], make_entry("foreign", json!({})));
    // 键序与缩进保留（定点换值不重排版）
    assert!(result.text.starts_with("{\n  \"models\": [\n    {\n      \"slug\": \"client-a\""));
}

#[test]
fn rewrite_appends_missing_entries_from_the_template() {
    let before = catalog_text(vec![make_entry(
        "client-a",
        json!({ "priority": 1001, "base_instructions": "模板的指令" }),
    )]);
    let result = rewrite_catalog(
        &before,
        &[want("client-a", json!({})), want("client-b", json!({ "reasoning": false }))],
    )
    .unwrap();
    let after: Value = serde_json::from_str(&result.text).unwrap();
    let models = after["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    let added = &models[1];
    assert_eq!(added["slug"], json!("client-b"));
    assert_eq!(added["display_name"], json!("client-b"));
    assert_eq!(added["priority"], json!(1001)); // 沿用模板
    assert_eq!(added["base_instructions"], json!("模板的指令")); // 沿用模板
    assert_eq!(added["supported_reasoning_levels"], json!([])); // 没勾推理就没有档位
}

#[test]
fn rewrite_from_empty_catalog_builds_required_fields() {
    let result = rewrite_catalog("{\n  \"models\": []\n}\n", &[want("client-a", json!({}))]).unwrap();
    let after: Value = serde_json::from_str(&result.text).unwrap();
    let entry = &after["models"][0];
    assert_eq!(entry["slug"], json!("client-a"));
    let object: &Map<String, Value> = entry.as_object().unwrap();
    for key in [
        "slug",
        "display_name",
        "supported_reasoning_levels",
        "shell_type",
        "visibility",
        "supported_in_api",
        "priority",
        "support_verbosity",
        "truncation_policy",
        "experimental_supported_tools",
        "base_instructions",
    ] {
        assert!(object.contains_key(key), "缺必需字段 {key}");
    }
}

#[test]
fn rewrite_removes_duplicate_entries_for_our_slug() {
    let before = catalog_text(vec![
        make_entry("client-a", json!({ "context_window": 999 })),
        make_entry("client-a", json!({ "context_window": 888 })),
        make_entry("foreign", json!({})),
    ]);
    let result = rewrite_catalog(&before, &[want("client-a", json!({}))]).unwrap();
    let after: Value = serde_json::from_str(&result.text).unwrap();
    let models = after["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0]["slug"], json!("client-a"));
    assert_eq!(models[0]["context_window"], json!(1024000));
    assert_eq!(models[1]["slug"], json!("foreign"));
}

