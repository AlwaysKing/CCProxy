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
    assert_eq!(ids, vec!["anthropic", "openai_chat", "openai_responses"]);
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
    assert_eq!(reloaded.config.providers[0].id, "p_zhipu");
    // 老结构（供应商挂在协议下）迁移后按归属协议推断上游格式
    assert_eq!(reloaded.config.providers[0].api_format, "anthropic");
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
    let providers: Vec<&str> = config.providers.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(providers, vec!["ok"]);
    let mappings: Vec<&str> = config.protocols[0].mappings.iter().map(|item| item.id.as_str()).collect();
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
    let models = &result.config.unwrap().providers[0].models;
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
    let models = &result.config.unwrap().providers[0].models;
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
    let serialized: Value = serde_json::to_value(&result.config.unwrap().providers[0].models).unwrap();
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
    let ids: Vec<&str> = config.protocols.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["anthropic", "openai_chat", "openai_responses"]);
    assert_eq!(config.protocols[1].mappings[0].client_model, "gpt-5*");
    assert_eq!(config.protocols[2].mappings[0].client_model, "gpt-5*");

    // 供应商升到顶层；格式按归属协议推断：Anthropic 那条明确，老的 openai 那条只能取 auto
    let formats: Vec<(&str, &str)> = config
        .providers
        .iter()
        .map(|item| (item.id.as_str(), item.api_format.as_str()))
        .collect();
    assert_eq!(formats, vec![("p_claude", "anthropic"), ("p_codex", "auto")]);

    // 迁移要有交代，但不能把老的 openai 条目报成「不支持的协议」
    assert!(result.issues.iter().any(|issue| issue.contains("供应商已升为全局")));
    assert!(result.issues.iter().any(|issue| issue.contains("OpenAI 协议已拆成")));
    assert!(!result.issues.iter().any(|issue| issue.contains("丢弃不支持的协议")));
}

#[test]
fn unknown_api_format_falls_back_to_auto() {
    let result = validate_config(&json!({
        "providers": [
            { "id": "p1", "name": "p1", "baseUrl": "http://a", "apiFormat": "gemini", "models": ["m"] }
        ]
    }));
    let config = result.config.unwrap();
    assert_eq!(config.providers[0].api_format, "auto");
    assert!(result.issues.iter().any(|issue| issue.contains("apiFormat")));
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
    assert_eq!(config.providers[0].api_format, "auto");
    assert_eq!(config.protocols[2].mappings.len(), 1);
    // 新结构下不该出现迁移提示
    assert!(!result.issues.iter().any(|issue| issue.contains("已升为全局")));
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
    assert!(result.issues.iter().any(|issue| issue.contains("已拆成 p1_2")));
}

#[test]
fn same_id_with_different_inferred_format_is_not_merged() {
    // 地址与 token 一样，但一个在老 anthropic 协议下（推断成 anthropic 格式）、一个在老 openai
    // 协议下（推断成 auto）。合并会让其中一侧从直通变成转换，属于静默的行为变化，所以照样拆开
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
    let formats: Vec<(&str, &str)> = config
        .providers
        .iter()
        .map(|item| (item.id.as_str(), item.api_format.as_str()))
        .collect();
    assert_eq!(formats, vec![("p1", "anthropic"), ("p1_2", "auto")]);
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
    assert!(result.issues.iter().any(|issue| issue.contains("老条目及其映射已被忽略")));
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
    assert_eq!(std::fs::read_to_string(dir.join("app.json.v1.bak")).unwrap(), legacy_text);

    // 落盘的是新结构：供应商在顶层、协议下没有 providers、版本号 2，其余设置原样保留
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], json!(2));
    assert_eq!(saved["providers"][0]["apiFormat"], json!("anthropic"));
    assert!(saved["protocols"][0].get("providers").is_none());
    assert_eq!(saved["settings"]["port"], json!(19000));
    assert_eq!(saved["settings"]["clientToken"], json!("sk-token"));

    // 再读一次不再触发升级（幂等），内容也不变
    let again = manager.load();
    assert!(!again.issues.iter().any(|issue| issue.contains("升级为新结构")));
    assert_eq!(again.config.providers.len(), 1);
    assert_eq!(again.config.protocols[0].mappings.len(), 1);
}

#[test]
fn protocol_names_come_from_the_presets() {
    // 老配置里那条 openai 的 name 是 "OpenAI"：拆成两条后如果继承它，
    // 顶栏 Tab 上会出现两个一模一样的「OpenAI」，分不出在配哪条
    let result = validate_config(&json!({
        "protocols": [{ "id": "openai", "name": "OpenAI", "enabled": true, "providers": [], "mappings": [] }]
    }));
    let config = result.config.unwrap();
    let names: Vec<&str> = config.protocols.iter().map(|item| item.name.as_str()).collect();
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
    let ids: Vec<&str> = config.providers.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["p_new"]);
    assert_eq!(config.protocols[0].mappings.len(), 1);
    assert!(result.issues.iter().any(|issue| issue.contains("以顶层为准")));
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

