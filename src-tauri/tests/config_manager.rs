// 配置管理单测：默认配置、原子写、损坏回落、校验归一化（含模型配置条目）

use serde_json::{json, Value};

use ccproxy_lib::config::{create_client_token, create_default_config, validate_config, ConfigManager, DEFAULT_PORT};

use common::{config_from, TempDir};

mod common;

fn manager(dir: &TempDir) -> ConfigManager {
    ConfigManager::new(Some(dir.path.clone()))
}

fn provider(id: &str, models: Value) -> Value {
    json!({ "id": id, "name": id, "enabled": true, "baseUrl": format!("https://{id}.example.com"), "token": "sk-x", "models": models })
}

fn mapping(id: &str, client_model: &str, provider_id: &str) -> Value {
    json!({ "id": id, "enabled": true, "clientModel": client_model, "providerId": provider_id, "realModel": "glm-4.6" })
}

#[test]
fn first_run_returns_defaults() {
    let dir = TempDir::new("ccproxy-cfg");
    let result = manager(&dir).load();
    assert!(result.first_run);
    assert!(!result.recovered);
    assert_eq!(result.config.settings.port, DEFAULT_PORT);
    let ids: Vec<&str> = result.config.protocols.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["anthropic", "openai"]);
}

#[test]
fn saved_config_round_trips() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let config = config_from(json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [provider("p_zhipu", json!(["glm-4.6"]))],
            "mappings": [mapping("m1", "claude-*", "p_zhipu")]
        }]
    }));
    manager.save(&serde_json::to_value(&config).unwrap()).unwrap();

    let reloaded = manager.load();
    assert_eq!(reloaded.config.protocols[0].providers[0].id, "p_zhipu");
    assert_eq!(reloaded.config.protocols[0].mappings[0].client_model, "claude-*");
}

#[test]
fn save_is_atomic_with_600_mode() {
    let dir = TempDir::new("ccproxy-cfg");
    manager(&dir).save(&serde_json::to_value(create_default_config()).unwrap()).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["app.json".to_string()]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("app.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn broken_json_is_backed_up_and_recovered() {
    let dir = TempDir::new("ccproxy-cfg");
    dir.write("app.json", "{ 这不是 json");
    let result = manager(&dir).load();
    assert!(result.recovered);
    let backup = result.backup_path.expect("应有备份路径");
    assert!(backup.ends_with("app.json.bak"));
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{ 这不是 json");
    assert_eq!(result.config.settings.port, DEFAULT_PORT);
}

#[test]
fn non_object_root_is_treated_as_broken() {
    let dir = TempDir::new("ccproxy-cfg");
    dir.write("app.json", "[1,2,3]");
    let result = manager(&dir).load();
    assert!(result.recovered);
    assert!(!validate_config(&json!([1, 2, 3])).ok);
}

#[test]
fn invalid_port_falls_back_with_issue() {
    let result = validate_config(&json!({ "settings": { "port": "abc" }, "protocols": [] }));
    assert!(result.ok);
    assert_eq!(result.config.unwrap().settings.port, DEFAULT_PORT);
    assert!(result.issues.iter().any(|issue| issue.contains("port")));
}

#[test]
fn unknown_protocol_is_dropped_with_issue() {
    let result = validate_config(&json!({
        "settings": { "port": 19000 },
        "protocols": [
            { "id": "gemini", "name": "Gemini", "enabled": true, "providers": [], "mappings": [] },
            { "id": "anthropic", "name": "Anthropic", "enabled": true, "providers": [], "mappings": [] }
        ]
    }));
    let config = result.config.unwrap();
    let ids: Vec<&str> = config.protocols.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["anthropic", "openai"]);
    assert!(result.issues.iter().any(|issue| issue.contains("gemini")));
}

#[test]
fn provider_without_base_url_and_mapping_without_provider_are_dropped() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [provider("ok", json!(["glm-4.6"])), { "id": "bad", "name": "bad", "token": "t", "models": [] }],
            "mappings": [mapping("m1", "a", "ok"), { "id": "m2", "clientModel": "b", "realModel": "x" }]
        }]
    }));
    let protocol = &result.config.unwrap().protocols[0];
    let providers: Vec<&str> = protocol.providers.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(providers, vec!["ok"]);
    let mappings: Vec<&str> = protocol.mappings.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(mappings, vec!["m1"]);
}

#[test]
fn mapping_to_missing_provider_is_dropped() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [provider("p1", json!(["glm-4.6"]))],
            "mappings": [mapping("m1", "a", "ghost")]
        }]
    }));
    assert_eq!(result.config.unwrap().protocols[0].mappings.len(), 0);
    assert!(result.issues.iter().any(|issue| issue.contains("ghost")));
}

#[test]
fn model_entries_accept_old_and_new_shapes() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "openai", "name": "OpenAI", "enabled": true,
            "providers": [provider("p1", json!([
                "glm-5.3",
                { "name": "gpt-5.5", "contextWindow": 400000, "maxContextWindow": 1024000, "inputModalities": ["text", "image"], "reasoning": true }
            ]))],
            "mappings": []
        }]
    }));
    let models = &result.config.unwrap().protocols[1].providers[0].models;
    let serialized: Value = serde_json::to_value(models).unwrap();
    assert_eq!(
        serialized,
        json!([
            { "name": "glm-5.3" },
            { "name": "gpt-5.5", "contextWindow": 400000, "maxContextWindow": 1024000, "inputModalities": ["text", "image"], "reasoning": true }
        ])
    );
}

#[test]
fn invalid_model_fields_drop_only_the_field() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [provider("p1", json!([
                { "name": "ok", "contextWindow": "abc", "maxContextWindow": 0, "inputModalities": ["text", "video"], "reasoning": "yes" },
                { "name": "typed", "contextWindow": true, "maxContextWindow": [400000] },
                { "name": "nulled", "inputModalities": null, "reasoning": null },
                { "name": "   " },
                null,
                { "name": "dup", "contextWindow": 111 },
                { "name": "dup", "contextWindow": 222 }
            ]))],
            "mappings": []
        }]
    }));
    let models = &result.config.unwrap().protocols[0].providers[0].models;
    let serialized: Value = serde_json::to_value(models).unwrap();
    assert_eq!(
        serialized,
        json!([
            { "name": "ok", "inputModalities": ["text"] },
            { "name": "typed" },
            { "name": "nulled" },
            { "name": "dup", "contextWindow": 111 }
        ])
    );
    assert!(!result.issues.iter().any(|issue| issue.contains("nulled")));
    for needle in ["contextWindow", "maxContextWindow", "inputModalities", "reasoning", "缺少 name", "非法模型条目", "重复模型 dup"] {
        assert!(
            result.issues.iter().any(|issue| issue.contains(needle)),
            "缺少 issue：{needle}（实际：{:?}）",
            result.issues
        );
    }
}

#[test]
fn blank_windows_are_not_written_back_and_modalities_are_deduped() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "openai", "name": "OpenAI", "enabled": true,
            "providers": [provider("p1", json!([
                { "name": "a", "contextWindow": "", "maxContextWindow": null, "inputModalities": ["text", "text", "image"] }
            ]))],
            "mappings": []
        }]
    }));
    let serialized: Value = serde_json::to_value(&result.config.unwrap().protocols[1].providers[0].models).unwrap();
    assert_eq!(serialized, json!([{ "name": "a", "inputModalities": ["text", "image"] }]));
}

#[test]
fn client_dirs_require_id_kind_absolute_path() {
    let good = json!({ "id": "d1", "kind": "claude", "path": "/Users/x/.claude-x" });
    let result = validate_config(&json!({
        "settings": {
            "clientDirs": [
                good,
                { "id": "d2", "kind": "codex", "path": "relative/.codex" },
                { "id": "d3", "kind": "gemini", "path": "/Users/x/.gemini" },
                { "kind": "claude", "path": "/Users/x/a" },
                { "id": "d4", "kind": "claude", "path": "/Users/x/.claude-x" },
                null
            ]
        }
    }));
    let dirs: Value = serde_json::to_value(&result.config.unwrap().settings.client_dirs).unwrap();
    assert_eq!(dirs, json!([good]));
    for needle in ["绝对路径", "kind 非法", "缺少 id", "重复目录"] {
        assert!(
            result.issues.iter().any(|issue| issue.contains(needle)),
            "缺少 issue：{needle}（实际：{:?}）",
            result.issues
        );
    }
}

#[test]
fn client_dirs_not_array_is_reset() {
    let broken = validate_config(&json!({ "settings": { "clientDirs": "nope" } }));
    assert!(broken.config.unwrap().settings.client_dirs.is_empty());
    assert!(broken.issues.iter().any(|issue| issue.contains("clientDirs")));
    let absent = validate_config(&json!({ "protocols": [] }));
    assert!(absent.config.unwrap().settings.client_dirs.is_empty());
}

#[test]
fn client_dirs_round_trip() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let mut config = create_default_config();
    config.settings.client_dirs = serde_json::from_value(json!([
        { "id": "d9", "kind": "codex", "path": "/Users/x/codex-config" }
    ]))
    .unwrap();
    manager.save(&serde_json::to_value(&config).unwrap()).unwrap();
    let reloaded = manager.load();
    let dirs: Value = serde_json::to_value(&reloaded.config.settings.client_dirs).unwrap();
    assert_eq!(dirs, json!([{ "id": "d9", "kind": "codex", "path": "/Users/x/codex-config" }]));
}

#[test]
fn client_token_is_trimmed_and_invalid_values_become_empty() {
    let kept = validate_config(&json!({ "settings": { "clientToken": "  t_abc  " } }));
    assert_eq!(kept.config.unwrap().settings.client_token, "t_abc");

    for bad in [Value::Null, json!(42), json!({ "a": 1 })] {
        let result = validate_config(&json!({ "settings": { "clientToken": bad } }));
        assert_eq!(result.config.unwrap().settings.client_token, "");
    }
    assert_eq!(validate_config(&json!({})).config.unwrap().settings.client_token, "");
}

#[test]
fn active_provider_id_is_not_kept() {
    let result = validate_config(&json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "activeProviderId": "p1",
            "providers": [provider("p1", json!(["glm-4.6"]))],
            "mappings": [mapping("m9", "*", "p1")]
        }]
    }));
    let serialized = serde_json::to_value(&result.config.unwrap().protocols[0]).unwrap();
    assert!(serialized.get("activeProviderId").is_none());
    assert_eq!(serialized["mappings"][0]["clientModel"], json!("*"));
}

#[test]
fn client_token_shape_and_uniqueness() {
    let token = create_client_token();
    assert!(token.starts_with("sk-"));
    assert_eq!(token.len(), 35);
    assert!(token[3..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(token, create_client_token());
}

#[test]
fn default_log_and_usage_switches_are_on() {
    let config = create_default_config();
    assert!(config.settings.logs_enabled);
    assert!(config.settings.usage_enabled);
}

#[test]
fn log_and_usage_switches_default_to_on() {
    let absent = validate_config(&json!({ "protocols": [] })).config.unwrap();
    assert!(absent.settings.logs_enabled);
    assert!(absent.settings.usage_enabled);

    let off = validate_config(&json!({ "settings": { "logsEnabled": false, "usageEnabled": false } }))
        .config
        .unwrap();
    assert!(!off.settings.logs_enabled);
    assert!(!off.settings.usage_enabled);

    // 手改配置塞进非布尔值：按「不等于 false」处理，仍是开启
    let weird = validate_config(&json!({ "settings": { "logsEnabled": "no", "usageEnabled": 0 } }))
        .config
        .unwrap();
    assert!(weird.settings.logs_enabled);
    assert!(weird.settings.usage_enabled);
}

