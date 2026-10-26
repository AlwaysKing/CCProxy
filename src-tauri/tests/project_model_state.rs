// 项目模型的状态层：配置事务、开启/关闭和映射变化触发的同步范围。

use serde_json::json;

use ccproxy_lib::clients::project_models::ProjectSyncState;
use ccproxy_lib::config::AppConfig;

use common::{config_from, test_state, TempDir};

mod common;

fn routing_config(project: &TempDir, enabled: bool) -> AppConfig {
    config_from(json!({
        "settings": {
            "projectModelSyncEnabled": enabled,
            "projectModelProjects": [{
                "id": "p1",
                "path": project.path_string(),
                "codexModel": "codex-a",
                "manual": true,
                "hidden": false
            }]
        },
        "providers": [{
            "id": "p1",
            "name": "provider",
            "enabled": true,
            "baseUrl": "https://example.com",
            "token": "sk-test",
            "modelGroups": [{
                "id": "g1",
                "apiFormat": "openai",
                "models": [{ "name": "real-codex" }]
            }]
        }],
        "protocols": [{
            "id": "openai_responses",
            "name": "OpenAI Responses",
            "enabled": true,
            "mappings": [{
                "id": "m1",
                "enabled": true,
                "clientModel": "codex-a",
                "providerId": "p1",
                "modelGroupId": "g1",
                "upstreamProtocol": "openai_responses",
                "realModel": "real-codex"
            }]
        }]
    }))
}

#[test]
fn setting_a_project_model_immediately_writes_it() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(&project, true), &state_dir);

    let result = state
        .set_project_model(&project.path_string(), "codex", "codex-a")
        .unwrap();
    assert_eq!(result.status, ProjectSyncState::Ok);
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"codex-a\"\n"
    );
}

#[test]
fn mapping_changes_rewrite_valid_bound_projects() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(&project, true), &state_dir);
    state
        .set_project_model(&project.path_string(), "codex", "codex-a")
        .unwrap();
    project.write(".codex/config.toml", "model = \"manual\"\n");

    let mut next = state.config_snapshot();
    next.protocols
        .iter_mut()
        .find(|protocol| protocol.id == "openai_responses")
        .unwrap()
        .mappings[0]
        .real_model = "real-codex-v2".to_string();
    state
        .apply_config(&serde_json::to_value(next).unwrap())
        .unwrap();
    assert!(std::fs::read_to_string(project.join(".codex/config.toml"))
        .unwrap()
        .contains("codex-a"));
}

#[test]
fn disabling_project_sync_stops_followup_writes() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(&project, true), &state_dir);
    state
        .set_project_model(&project.path_string(), "codex", "codex-a")
        .unwrap();
    project.write(".codex/config.toml", "model = \"manual\"\n");
    state.set_project_sync_enabled(false).unwrap();

    let mut next = state.config_snapshot();
    next.protocols
        .iter_mut()
        .find(|protocol| protocol.id == "openai_responses")
        .unwrap()
        .mappings[0]
        .client_model = "codex-b".to_string();
    next.settings.project_model_projects[0].codex_model = Some("codex-b".to_string());
    state
        .apply_config(&serde_json::to_value(next).unwrap())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"manual\"\n"
    );
}

#[test]
fn enabling_project_sync_writes_existing_bindings() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(&project, false), &state_dir);
    assert!(!project.join(".codex/config.toml").exists());

    state.set_project_sync_enabled(true).unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"codex-a\"\n"
    );
}
