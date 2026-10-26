// 项目级默认模型：发现、候选校验和只写 model 的同步行为。

use serde_json::json;

use ccproxy_lib::clients::project_models::{
    apply_project_model, discover_projects, project_model_candidates, ProjectSyncState,
};
use ccproxy_lib::config::AppConfig;

use common::{config_from, TempDir};

mod common;

fn routing_config() -> AppConfig {
    config_from(json!({
        "providers": [{
            "id": "p1",
            "name": "provider",
            "enabled": true,
            "baseUrl": "https://example.com",
            "token": "sk-test",
            "modelGroups": [
                {
                    "id": "g-claude",
                    "apiFormat": "anthropic",
                    "models": [{ "name": "real-claude" }]
                },
                {
                    "id": "g-codex",
                    "apiFormat": "openai",
                    "models": [{ "name": "real-codex" }]
                }
            ]
        }],
        "protocols": [
            {
                "id": "anthropic",
                "name": "Anthropic",
                "enabled": true,
                "mappings": [{
                    "id": "m-claude",
                    "enabled": true,
                    "clientModel": "claude-alias",
                    "providerId": "p1",
                    "modelGroupId": "g-claude",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real-claude"
                }, {
                    "id": "m-claude-wild",
                    "enabled": true,
                    "clientModel": "claude-*",
                    "providerId": "p1",
                    "modelGroupId": "g-claude",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real-claude"
                }]
            },
            {
                "id": "openai_responses",
                "name": "OpenAI Responses",
                "enabled": true,
                "mappings": [{
                    "id": "m-codex",
                    "enabled": true,
                    "clientModel": "codex-alias",
                    "providerId": "p1",
                    "modelGroupId": "g-codex",
                    "upstreamProtocol": "openai_responses",
                    "realModel": "real-codex"
                }, {
                    "id": "m-codex-wild",
                    "enabled": true,
                    "clientModel": "codex-*",
                    "providerId": "p1",
                    "modelGroupId": "g-codex",
                    "upstreamProtocol": "openai_responses",
                    "realModel": "real-codex"
                }]
            }
        ]
    }))
}

#[test]
fn discovery_merges_codex_and_claude_for_one_project() {
    let home = TempDir::new("ccproxy-project-home");
    let project = home.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let project_path = std::fs::canonicalize(&project)
        .unwrap()
        .to_string_lossy()
        .to_string();

    home.write(
        ".codex/sessions/2026/10/26/session.jsonl",
        &format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": { "cwd": project_path, "timestamp": "2026-10-26T01:00:00Z" }
            })
        ),
    );
    home.write(
        ".claude/history.jsonl",
        &format!(
            "{}\n",
            json!({ "project": project_path, "timestamp": 1_790_000_000_000i64 })
        ),
    );

    let projects = discover_projects(&home.path);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].path, project_path);
    assert!(projects[0].sources.codex);
    assert!(projects[0].sources.claude);
    assert!(projects[0].last_activity_ms.is_some());
}

#[test]
fn model_candidates_use_exact_enabled_mappings_only() {
    let candidates = project_model_candidates(&routing_config());
    assert_eq!(candidates.codex, vec!["codex-alias"]);
    assert_eq!(candidates.claude, vec!["claude-alias"]);
}

#[test]
fn codex_binding_creates_only_the_project_model() {
    let project = TempDir::new("ccproxy-project-codex");
    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "codex",
        "codex-alias",
    );
    assert_eq!(result.status, ProjectSyncState::Ok);
    assert!(result.changed);
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"codex-alias\"\n"
    );
}

#[test]
fn claude_binding_creates_only_the_project_model() {
    let project = TempDir::new("ccproxy-project-claude");
    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::Ok);
    assert!(result.changed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(project.join(".claude/settings.local.json")).unwrap()
        )
        .unwrap(),
        json!({ "model": "claude-alias" })
    );
}

#[test]
fn invalid_mapping_never_overwrites_the_existing_model() {
    let project = TempDir::new("ccproxy-project-invalid");
    project.write(".codex/config.toml", "model = \"existing\"\n");
    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "codex",
        "not-configured",
    );
    assert_eq!(result.status, ProjectSyncState::BindingInvalid);
    assert!(!result.changed);
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"existing\"\n"
    );
}

#[test]
fn missing_project_is_reported_without_creating_directories() {
    let root = TempDir::new("ccproxy-project-missing");
    let missing = root.join("gone");
    let result = apply_project_model(
        &routing_config(),
        &missing.to_string_lossy(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::ProjectMissing);
    assert!(!missing.exists());
}
