// 配置管理单测：默认配置、原子写、损坏回落、校验归一化（含模型配置条目）

use serde_json::{json, Value};

use ccproxy_lib::config::{
    create_client_token, create_default_config, validate_config, ConfigManager, CONFIG_VERSION,
    DEFAULT_PORT,
};

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
    let ids: Vec<&str> = result
        .config
        .protocols
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(ids, vec!["anthropic", "openai_chat", "openai_responses"]);
}

#[test]
fn legacy_project_models_expand_to_flat_bindings_and_keep_offline_paths() {
    let result = validate_config(&json!({
        "version": 5,
        "settings": {
            "projectModelProjects": [{
                "id": "p1",
                "path": "/tmp/ccproxy-project-that-does-not-need-to-exist",
                "codexModel": "codex-model",
                "claudeModel": "claude-model",
                "manual": true,
                "hidden": false
            }]
        }
    }));
    let config = result.config.unwrap();
    assert_eq!(config.version, 6);
    assert_eq!(config.settings.project_model_bindings.len(), 2);
    assert_eq!(
        config.settings.project_model_bindings[0].path,
        "/tmp/ccproxy-project-that-does-not-need-to-exist"
    );
    assert_eq!(config.settings.project_model_bindings[0].client, "codex");
    assert_eq!(config.settings.project_model_bindings[1].client, "claude");
}

#[test]
fn project_model_paths_and_ids_are_normalized() {
    let result = validate_config(&json!({
        "settings": {
            "projectModelBindings": [
                {
                    "id": "same",
                    "path": "/tmp/project-a",
                    "client": "codex",
                    "model": "m1"
                },
                {
                    "id": "same",
                    "path": "/tmp/project-b",
                    "client": "claude",
                    "model": "m2"
                },
                {
                    "id": "duplicate-path",
                    "path": "/tmp/project-a/",
                    "client": "codex",
                    "model": "m3"
                }
            ]
        }
    }));
    let config = result.config.unwrap();
    let bindings = config.settings.project_model_bindings;
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].path, "/tmp/project-a");
    assert_eq!(bindings[0].client, "codex");
    assert_eq!(bindings[0].model, "m1");
    assert_eq!(bindings[1].path, "/tmp/project-b");
    assert_ne!(bindings[0].id, bindings[1].id);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("重复的项目绑定")));
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("重复的项目绑定 id")));
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
    manager
        .save(&serde_json::to_value(&config).unwrap())
        .unwrap();

    let reloaded = manager.load();
    assert_eq!(reloaded.config.providers[0].id, "p_zhipu");
    // 老结构（供应商挂在协议下）迁移后按归属协议生成模型组
    assert_eq!(reloaded.config.providers[0].model_groups.len(), 1);
    assert_eq!(
        reloaded.config.providers[0].model_groups[0].api_format,
        "anthropic"
    );
    assert_eq!(
        reloaded.config.protocols[0].mappings[0].client_model,
        "claude-*"
    );
}

#[test]
fn save_is_atomic_with_600_mode() {
    let dir = TempDir::new("ccproxy-cfg");
    manager(&dir)
        .save(&serde_json::to_value(create_default_config()).unwrap())
        .unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["app.json".to_string()]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("app.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
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
    let ids: Vec<&str> = config
        .protocols
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(ids, vec!["anthropic", "openai_chat", "openai_responses"]);
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
    let config = result.config.unwrap();
    let providers: Vec<&str> = config
        .providers
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(providers, vec!["ok"]);
    let mappings: Vec<&str> = config.protocols[0]
        .mappings
        .iter()
        .map(|item| item.id.as_str())
        .collect();
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
    let models = &result.config.unwrap().providers[0].model_groups[0].models;
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
fn v2_openai_provider_expands_to_groups_and_keeps_capabilities() {
    let result = validate_config(&json!({
        "version": 2,
        "providers": [{
            "id": "p1",
            "name": "OpenAI",
            "enabled": true,
            "apiFormat": "auto",
            "baseUrl": "http://openai",
            "token": "sk-x",
            "models": [{
                "name": "gpt-5.5",
                "contextWindow": 400000,
                "inputModalities": ["text", "image"],
                "reasoning": true
            }]
        }],
        "protocols": [
            {
                "id": "openai_chat",
                "enabled": true,
                "mappings": [mapping("m_chat", "gpt-5*", "p1")]
            },
            {
                "id": "openai_responses",
                "enabled": true,
                "mappings": [mapping("m_responses", "gpt-5*", "p1")]
            }
        ]
    }));
    let config = result.config.unwrap();
    let provider = &config.providers[0];
    let formats: Vec<&str> = provider
        .model_groups
        .iter()
        .map(|group| group.api_format.as_str())
        .collect();
    assert_eq!(formats, vec!["openai"]);
    for group in &provider.model_groups {
        let serialized = serde_json::to_value(&group.models).unwrap();
        assert_eq!(
            serialized,
            json!([{
                "name": "gpt-5.5",
                "contextWindow": 400000,
                "inputModalities": ["text", "image"],
                "reasoning": true
            }])
        );
    }
    assert_eq!(config.protocols[1].mappings[0].model_group_id, "g_openai");
    assert_eq!(config.protocols[2].mappings[0].model_group_id, "g_openai");
    assert_eq!(
        config.protocols[1].mappings[0].upstream_protocol,
        "openai_chat"
    );
    assert_eq!(
        config.protocols[2].mappings[0].upstream_protocol,
        "openai_responses"
    );
}

#[test]
fn mapping_keeps_an_explicit_model_group() {
    let result = validate_config(&json!({
        "providers": [{
            "id": "p1",
            "name": "共享供应商",
            "enabled": true,
            "baseUrl": "http://shared",
            "token": "t",
            "modelGroups": [
                {
                    "id": "g_chat",
                    "apiFormat": "openai_chat",
                    "models": [{ "name": "gpt-5.5" }]
                },
                {
                    "id": "g_responses",
                    "apiFormat": "openai_responses",
                    "models": [{ "name": "gpt-5.5" }]
                }
            ]
        }],
        "protocols": [{
            "id": "openai_responses",
            "enabled": true,
            "mappings": [{
                "id": "m1",
                "enabled": true,
                "clientModel": "gpt-5.5",
                "providerId": "p1",
                "modelGroupId": "g_chat",
                "upstreamProtocol": "openai_responses",
                "realModel": "gpt-5.5"
            }]
        }]
    }));
    let mapping = &result.config.unwrap().protocols[2].mappings[0];
    assert_eq!(mapping.model_group_id, "g_openai");
    assert_eq!(mapping.upstream_protocol, "openai_responses");
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
    let models = &result.config.unwrap().providers[0].model_groups[0].models;
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
    for needle in [
        "contextWindow",
        "maxContextWindow",
        "inputModalities",
        "reasoning",
        "缺少 name",
        "非法模型条目",
        "重复模型 dup",
    ] {
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
    let serialized: Value =
        serde_json::to_value(&result.config.unwrap().providers[0].model_groups[0].models).unwrap();
    assert_eq!(
        serialized,
        json!([{ "name": "a", "inputModalities": ["text", "image"] }])
    );
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
    assert!(broken
        .issues
        .iter()
        .any(|issue| issue.contains("clientDirs")));
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
    manager
        .save(&serde_json::to_value(&config).unwrap())
        .unwrap();
    let reloaded = manager.load();
    let dirs: Value = serde_json::to_value(&reloaded.config.settings.client_dirs).unwrap();
    assert_eq!(
        dirs,
        json!([{ "id": "d9", "kind": "codex", "path": "/Users/x/codex-config" }])
    );
}

#[test]
fn client_token_is_trimmed_and_invalid_values_become_empty() {
    let kept = validate_config(&json!({ "settings": { "clientToken": "  t_abc  " } }));
    assert_eq!(kept.config.unwrap().settings.client_token, "t_abc");

    for bad in [Value::Null, json!(42), json!({ "a": 1 })] {
        let result = validate_config(&json!({ "settings": { "clientToken": bad } }));
        assert_eq!(result.config.unwrap().settings.client_token, "");
    }
    assert_eq!(
        validate_config(&json!({}))
            .config
            .unwrap()
            .settings
            .client_token,
        ""
    );
}

#[test]
fn legacy_config_hoists_providers_and_splits_openai() {
    let result = validate_config(&json!({
        "protocols": [
            {
                "id": "anthropic", "name": "Anthropic", "enabled": true,
                "providers": [provider("p_claude", json!(["glm-4.6"]))],
                "mappings": [mapping("m1", "claude-*", "p_claude")]
            },
            {
                "id": "openai", "name": "OpenAI", "enabled": true,
                "providers": [provider("p_codex", json!(["gpt-5.5"]))],
                "mappings": [mapping("m2", "gpt-5*", "p_codex")]
            }
        ]
    }));
    let config = result.config.unwrap();

    // OpenAI 拆成两条，原映射各复制一份
    let ids: Vec<&str> = config
        .protocols
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(ids, vec!["anthropic", "openai_chat", "openai_responses"]);
    assert_eq!(config.protocols[1].mappings[0].client_model, "gpt-5*");
    assert_eq!(config.protocols[2].mappings[0].client_model, "gpt-5*");

    // 供应商升到顶层；格式按归属协议推断：Anthropic 那条明确，老的 openai 那条只能取 auto
    let formats: Vec<Vec<&str>> = config
        .providers
        .iter()
        .map(|item| {
            item.model_groups
                .iter()
                .map(|group| group.api_format.as_str())
                .collect()
        })
        .collect();
    assert_eq!(formats, vec![vec!["anthropic"], vec!["openai"]]);
    assert_eq!(config.providers[1].model_groups.len(), 1);

    // 迁移要有交代，但不能把老的 openai 条目报成「不支持的协议」
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("供应商已升为全局")));
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("OpenAI 协议已拆成")));
    assert!(!result
        .issues
        .iter()
        .any(|issue| issue.contains("丢弃不支持的协议")));
}

#[test]
fn unknown_api_format_falls_back_to_legacy_openai_groups() {
    let result = validate_config(&json!({
        "providers": [
            { "id": "p1", "name": "p1", "baseUrl": "http://a", "apiFormat": "gemini", "models": ["m"] }
        ]
    }));
    let config = result.config.unwrap();
    let formats: Vec<&str> = config.providers[0]
        .model_groups
        .iter()
        .map(|group| group.api_format.as_str())
        .collect();
    assert_eq!(formats, vec!["openai"]);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("apiFormat")));
}

#[test]
fn new_shape_providers_are_read_from_the_top_level() {
    let result = validate_config(&json!({
        "providers": [provider("p_new", json!(["m"]))],
        "protocols": [{
            "id": "openai_responses", "enabled": true,
            "mappings": [mapping("m1", "gpt-5*", "p_new")]
        }]
    }));
    let config = result.config.unwrap();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].model_groups.len(), 1);
    assert_eq!(config.providers[0].model_groups[0].api_format, "openai");
    assert_eq!(config.protocols[2].mappings.len(), 1);
    // 新结构下不该出现迁移提示
    assert!(!result
        .issues
        .iter()
        .any(|issue| issue.contains("已升为全局")));
}

#[test]
fn colliding_provider_ids_are_split_and_mappings_follow() {
    let result = validate_config(&json!({
        "protocols": [
            {
                "id": "anthropic", "enabled": true,
                "providers": [{ "id": "p1", "name": "A家", "baseUrl": "http://a", "token": "t1", "models": ["m"] }],
                "mappings": [mapping("m1", "claude-*", "p1")]
            },
            {
                "id": "openai", "enabled": true,
                "providers": [{ "id": "p1", "name": "B家", "baseUrl": "http://b", "token": "t2", "models": ["m"] }],
                "mappings": [mapping("m2", "gpt-*", "p1")]
            }
        ]
    }));
    let config = result.config.unwrap();

    // 同一个 id 在老配置的两个协议下指向不同上游：拆成两条，不能悄悄合成一条
    let pairs: Vec<(&str, &str)> = config
        .providers
        .iter()
        .map(|item| (item.id.as_str(), item.base_url.as_str()))
        .collect();
    assert_eq!(pairs, vec![("p1", "http://a"), ("p1_2", "http://b")]);

    // 各协议内的映射按原来的指向改指：anthropic 仍指向 A 家，拆出来的两条指向 B 家
    assert_eq!(config.protocols[0].mappings[0].provider_id, "p1");
    assert_eq!(config.protocols[1].mappings[0].provider_id, "p1_2");
    assert_eq!(config.protocols[2].mappings[0].provider_id, "p1_2");
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("已拆成 p1_2")));
}

#[test]
fn same_id_with_different_inferred_format_is_not_merged() {
    // 地址与 token 一样，但一个在老 anthropic 协议下、一个在老 openai 协议下。
    // 两个协议对应的模型组不同，不能合并成同一组。
    let result = validate_config(&json!({
        "protocols": [
            {
                "id": "anthropic", "enabled": true,
                "providers": [{ "id": "p1", "baseUrl": "http://same", "token": "t", "models": ["m"] }],
                "mappings": []
            },
            {
                "id": "openai", "enabled": true,
                "providers": [{ "id": "p1", "baseUrl": "http://same", "token": "t", "models": ["m"] }],
                "mappings": []
            }
        ]
    }));
    let config = result.config.unwrap();
    let formats: Vec<Vec<&str>> = config
        .providers
        .iter()
        .map(|item| {
            item.model_groups
                .iter()
                .map(|group| group.api_format.as_str())
                .collect()
        })
        .collect();
    assert_eq!(formats, vec![vec!["anthropic"], vec!["openai"]]);
}

#[test]
fn legacy_entry_fills_the_empty_slot_and_is_otherwise_ignored() {
    let result = validate_config(&json!({
        "providers": [provider("p1", json!(["m"]))],
        "protocols": [
            { "id": "openai_chat", "enabled": true, "mappings": [mapping("m1", "gpt-4*", "p1")] },
            { "id": "openai", "enabled": true, "mappings": [mapping("m9", "legacy-*", "p1")] }
        ]
    }));
    let config = result.config.unwrap();

    // 拆分后的条目说了算：chat 那条用新的映射
    assert_eq!(config.protocols[1].mappings[0].client_model, "gpt-4*");
    // responses 那条没有自己的条目，从老的 openai 条目补上（总比空着强）
    assert_eq!(config.protocols[2].mappings[0].client_model, "legacy-*");
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("老条目及其映射已被忽略")));
}

#[test]
fn loading_a_legacy_file_backs_it_up_and_writes_the_new_shape() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let legacy = json!({
        "version": 1,
        "settings": { "port": 19000, "clientToken": "sk-token" },
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [provider("p_zhipu", json!(["glm-4.6"]))],
            "mappings": [mapping("m1", "claude-*", "p_zhipu")]
        }]
    });
    let legacy_text = serde_json::to_string_pretty(&legacy).unwrap();
    dir.write("app.json", &legacy_text);

    let loaded = manager.load();
    assert!(!loaded.recovered);
    assert_eq!(loaded.config.settings.port, 19000);
    assert_eq!(loaded.config.settings.client_token, "sk-token");
    assert_eq!(loaded.config.providers[0].id, "p_zhipu");
    assert!(loaded
        .issues
        .iter()
        .any(|issue| issue.contains("已把旧版配置升级为新结构")));

    // 原文件原样留了一份备份，用户手上还有能照着改回去的旧结构
    assert_eq!(
        std::fs::read_to_string(dir.join("app.json.v1.bak")).unwrap(),
        legacy_text
    );

    // 落盘的是新结构：供应商在顶层、协议下没有 providers、版本号升到当前版本，其余设置原样保留
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], json!(CONFIG_VERSION));
    assert_eq!(
        saved["providers"][0]["modelGroups"][0]["apiFormat"],
        json!("anthropic")
    );
    assert!(saved["protocols"][0].get("providers").is_none());
    assert_eq!(saved["settings"]["port"], json!(19000));
    assert_eq!(saved["settings"]["clientToken"], json!("sk-token"));

    // 再读一次不再触发升级（幂等），内容也不变
    let again = manager.load();
    assert!(!again
        .issues
        .iter()
        .any(|issue| issue.contains("升级为新结构")));
    assert_eq!(again.config.providers.len(), 1);
    assert_eq!(again.config.protocols[0].mappings.len(), 1);
}

#[test]
fn loading_a_v2_file_backs_up_and_expands_models_without_losing_abilities() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let legacy = json!({
        "version": 2,
        "settings": { "port": 19000 },
        "providers": [{
            "id": "p_openai",
            "name": "OpenAI",
            "enabled": true,
            "apiFormat": "auto",
            "baseUrl": "https://openai.example.com/v1",
            "token": "sk-x",
            "models": [{
                "name": "gpt-5.5",
                "contextWindow": 400000,
                "maxContextWindow": 1024000,
                "inputModalities": ["text", "image"],
                "reasoning": true
            }]
        }],
        "protocols": [
            {
                "id": "openai_chat",
                "enabled": true,
                "mappings": [mapping("m_chat", "gpt-5*", "p_openai")]
            },
            {
                "id": "openai_responses",
                "enabled": true,
                "mappings": [mapping("m_responses", "gpt-5*", "p_openai")]
            }
        ]
    });
    let legacy_text = serde_json::to_string_pretty(&legacy).unwrap();
    dir.write("app.json", &legacy_text);

    let loaded = manager.load();
    assert!(!loaded.recovered);
    assert_eq!(loaded.config.providers[0].model_groups.len(), 1);
    for group in &loaded.config.providers[0].model_groups {
        assert_eq!(group.models[0].context_window, Some(400000));
        assert_eq!(group.models[0].reasoning, Some(true));
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("app.json.v2.bak")).unwrap(),
        legacy_text
    );

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], json!(CONFIG_VERSION));
    assert_eq!(
        saved["providers"][0]["modelGroups"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(saved["providers"][0].get("apiFormat").is_none());
}

#[test]
fn loading_intermediate_v3_promotes_group_address_and_token() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let intermediate = json!({
        "version": 3,
        "settings": { "port": 19000 },
        "providers": [{
            "id": "p1",
            "name": "共享供应商",
            "enabled": true,
            "modelGroups": [
                {
                    "id": "g_chat",
                    "apiFormat": "openai_chat",
                    "baseUrl": "https://shared.example.com/v1",
                    "token": "sk-shared",
                    "models": [{ "name": "gpt-5.5", "contextWindow": 400000 }]
                },
                {
                    "id": "g_responses",
                    "apiFormat": "openai_responses",
                    "baseUrl": "https://shared.example.com/v1",
                    "token": "sk-shared",
                    "models": [{ "name": "gpt-5.6", "reasoning": true }]
                }
            ]
        }],
        "protocols": [{
            "id": "openai_responses",
            "enabled": true,
            "mappings": [{
                "id": "m1",
                "enabled": true,
                "clientModel": "gpt-5.5",
                "providerId": "p1",
                "modelGroupId": "g_responses",
                "realModel": "gpt-5.5"
            }]
        }]
    });
    let original = serde_json::to_string_pretty(&intermediate).unwrap();
    dir.write("app.json", &original);

    let loaded = manager.load();
    assert!(!loaded.recovered);
    assert_eq!(
        loaded.config.providers[0].base_url,
        "https://shared.example.com/v1"
    );
    assert_eq!(loaded.config.providers[0].token, "sk-shared");
    assert_eq!(loaded.config.providers[0].model_groups.len(), 1);
    assert_eq!(
        loaded.config.providers[0].model_groups[0].api_format,
        "openai"
    );
    assert_eq!(loaded.config.providers[0].model_groups[0].models.len(), 2);
    assert_eq!(
        loaded.config.providers[0].model_groups[0].models[0].context_window,
        Some(400000)
    );
    assert_eq!(
        loaded.config.providers[0].model_groups[0].models[1].reasoning,
        Some(true)
    );
    assert_eq!(
        loaded.config.protocols[2].mappings[0].model_group_id,
        "g_openai"
    );
    assert_eq!(
        loaded.config.protocols[2].mappings[0].upstream_protocol,
        "openai_responses"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("app.json.v3.bak")).unwrap(),
        original
    );

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    assert_eq!(
        saved["providers"][0]["baseUrl"],
        json!("https://shared.example.com/v1")
    );
    assert_eq!(saved["providers"][0]["token"], json!("sk-shared"));
    assert!(saved["providers"][0]["modelGroups"][0]
        .get("baseUrl")
        .is_none());
    assert!(saved["providers"][0]["modelGroups"][0]
        .get("token")
        .is_none());
}

#[test]
fn loading_v4_sorts_mappings_into_legacy_priority_order() {
    let dir = TempDir::new("ccproxy-cfg");
    let manager = manager(&dir);
    let legacy = json!({
        "version": 4,
        "settings": { "port": 19000 },
        "providers": [{
            "id": "p1", "name": "供应商", "enabled": true,
            "baseUrl": "https://p1.example.com", "token": "sk-x",
            "modelGroups": [{
                "id": "g1", "apiFormat": "anthropic",
                "models": [{ "name": "m1" }]
            }]
        }],
        "protocols": [{
            "id": "anthropic", "enabled": true,
            "mappings": [
                { "id": "wild-b", "enabled": true, "clientModel": "claude-b*", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" },
                { "id": "fallback", "enabled": true, "clientModel": "*", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" },
                { "id": "wild-short", "enabled": true, "clientModel": "claude-*", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" },
                { "id": "wild-long", "enabled": true, "clientModel": "claude-3-5-*", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" },
                { "id": "wild-a", "enabled": true, "clientModel": "claude-a*", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" },
                { "id": "exact", "enabled": true, "clientModel": "claude-3-5-sonnet", "providerId": "p1", "modelGroupId": "g_anthropic", "realModel": "m1" }
            ]
        }]
    });
    let legacy_text = serde_json::to_string_pretty(&legacy).unwrap();
    dir.write("app.json", &legacy_text);

    let loaded = manager.load();
    let ids: Vec<&str> = loaded.config.protocols[0]
        .mappings
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    // 旧语义：精确 > 通配（前缀长的在前）> 兜底。升级时排一次序，命中结果与升级前一致；
    // wild-a / wild-b 等长，保持原相对顺序（稳定排序）
    assert_eq!(
        ids,
        vec![
            "exact",
            "wild-long",
            "wild-b",
            "wild-a",
            "wild-short",
            "fallback"
        ]
    );
    assert!(loaded
        .issues
        .iter()
        .any(|issue| issue.contains("已把旧版配置升级为当前版本")));
    assert_eq!(
        std::fs::read_to_string(dir.join("app.json.v4.bak")).unwrap(),
        legacy_text
    );

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], json!(CONFIG_VERSION));
    // 落盘顺序也要是整理后的顺序，不能每次加载都重排
    let saved_ids: Vec<&str> = saved["protocols"][0]["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(saved_ids, ids);
}

#[test]
fn protocol_names_come_from_the_presets() {
    // 老配置里那条 openai 的 name 是 "OpenAI"：拆成两条后如果继承它，
    // 顶栏 Tab 上会出现两个一模一样的「OpenAI」，分不出在配哪条
    let result = validate_config(&json!({
        "protocols": [{ "id": "openai", "name": "OpenAI", "enabled": true, "providers": [], "mappings": [] }]
    }));
    let config = result.config.unwrap();
    let names: Vec<&str> = config
        .protocols
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, vec!["Anthropic", "OpenAI Chat", "OpenAI Responses"]);

    // 手改过的名字也不采纳：协议名是预设，不是配置项
    let hand_edited = validate_config(&json!({
        "protocols": [{ "id": "anthropic", "name": "我自己起的名字", "enabled": true, "mappings": [] }]
    }));
    assert_eq!(hand_edited.config.unwrap().protocols[0].name, "Anthropic");
}

#[test]
fn nested_providers_are_ignored_when_the_top_level_pool_exists() {
    let result = validate_config(&json!({
        "providers": [provider("p_new", json!(["m"]))],
        "protocols": [{
            "id": "anthropic", "enabled": true,
            "providers": [provider("p_stale", json!(["m2"]))],
            "mappings": [mapping("m1", "claude-*", "p_new")]
        }]
    }));
    let config = result.config.unwrap();
    // 顶层池是唯一真源：协议下那份老数据不会被提升回来，否则等于把已经删掉的供应商救活
    let ids: Vec<&str> = config
        .providers
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(ids, vec!["p_new"]);
    assert_eq!(config.protocols[0].mappings.len(), 1);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("以顶层为准")));
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

    let off =
        validate_config(&json!({ "settings": { "logsEnabled": false, "usageEnabled": false } }))
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
