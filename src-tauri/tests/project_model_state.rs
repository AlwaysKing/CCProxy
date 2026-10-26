// 项目模型状态层：按协议添加、编辑、移除和映射变化同步。

use serde_json::json;

use ccproxy_lib::clients::project_models::{DiscoveredProject, ProjectSources};
use ccproxy_lib::config::AppConfig;

use common::{config_from, test_state, TempDir};

mod common;

fn routing_config(bindings: Vec<serde_json::Value>) -> AppConfig {
    config_from(json!({
        "settings": {
            "projectModelBindings": bindings
        },
        "providers": [{
            "id": "p1",
            "name": "provider",
            "enabled": true,
            "baseUrl": "https://example.com",
            "token": "sk-test",
            "modelGroups": [
                {
                    "id": "g-codex",
                    "apiFormat": "openai",
                    "models": [{ "name": "real-codex" }]
                },
                {
                    "id": "g-claude",
                    "apiFormat": "anthropic",
                    "models": [{ "name": "real-claude" }]
                }
            ]
        }],
        "protocols": [
            {
                "id": "openai_responses",
                "name": "OpenAI Responses",
                "enabled": true,
                "mappings": [{
                    "id": "m1",
                    "enabled": true,
                    "clientModel": "codex-a",
                    "providerId": "p1",
                    "modelGroupId": "g-codex",
                    "upstreamProtocol": "openai_responses",
                    "realModel": "real-codex"
                }]
            },
            {
                "id": "anthropic",
                "name": "Anthropic",
                "enabled": true,
                "mappings": [{
                    "id": "m2",
                    "enabled": true,
                    "clientModel": "claude-a",
                    "providerId": "p1",
                    "modelGroupId": "g-claude",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real-claude"
                }]
            }
        ]
    }))
}

fn candidate(path: &str, codex: bool, claude: bool) -> DiscoveredProject {
    DiscoveredProject {
        path: path.to_string(),
        sources: ProjectSources { codex, claude },
        last_activity_ms: Some(1),
    }
}

#[tokio::test]
async fn batch_add_creates_flat_bindings_and_writes_files() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(Vec::new()), &state_dir);
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);

    state
        .add_project_bindings("openai_responses", "m1", vec![path.clone()])
        .await
        .unwrap();

    let bindings = state.config_snapshot().settings.project_model_bindings;
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].client, "codex");
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"codex-a\"\n"
    );
}

#[tokio::test]
async fn adding_claude_to_the_same_path_is_a_separate_binding() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(Vec::new()), &state_dir);
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, true)]);

    state
        .add_project_bindings("openai_responses", "m1", vec![path.clone()])
        .await
        .unwrap();
    state
        .add_project_bindings("anthropic", "m2", vec![path])
        .await
        .unwrap();

    let bindings = state.config_snapshot().settings.project_model_bindings;
    assert_eq!(bindings.len(), 2);
    assert!(bindings.iter().any(|binding| binding.client == "codex"));
    assert!(bindings.iter().any(|binding| binding.client == "claude"));
}

#[tokio::test]
async fn different_mappings_write_only_their_own_models() {
    let first = TempDir::new("ccproxy-state-first");
    let second = TempDir::new("ccproxy-state-second");
    let state_dir = TempDir::new("ccproxy-state-config");
    let config = routing_config(vec![json!({
        "id": "b1",
        "path": first.path_string(),
        "client": "codex",
        "mappingId": "m1"
    })]);
    let state = test_state(config, &state_dir);
    let first_path = std::fs::canonicalize(&first.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let second_path = std::fs::canonicalize(&second.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![
        candidate(&first_path, true, false),
        candidate(&second_path, true, false),
    ]);
    let mut next = state.config_snapshot();
    next.protocols
        .iter_mut()
        .find(|protocol| protocol.id == "openai_responses")
        .unwrap()
        .mappings
        .push(
            serde_json::from_value(json!({
                "id": "m3",
                "enabled": true,
                "clientModel": "codex-b",
                "providerId": "p1",
                "modelGroupId": "g-codex",
                "upstreamProtocol": "openai_responses",
                "realModel": "real-codex"
            }))
            .unwrap(),
        );
    state
        .apply_config(&serde_json::to_value(next).unwrap())
        .unwrap();
    state
        .add_project_bindings("openai_responses", "m3", vec![second_path])
        .await
        .unwrap();

    let mut next = state.config_snapshot();
    next.protocols
        .iter_mut()
        .find(|protocol| protocol.id == "openai_responses")
        .unwrap()
        .mappings[0]
        .client_model = "codex-c".to_string();
    state
        .apply_config(&serde_json::to_value(next).unwrap())
        .unwrap();

    assert!(std::fs::read_to_string(first.join(".codex/config.toml"))
        .unwrap()
        .contains("codex-c"));
    assert!(std::fs::read_to_string(second.join(".codex/config.toml"))
        .unwrap()
        .contains("codex-b"));
}

#[tokio::test]
async fn removing_a_binding_keeps_the_project_file() {
    let project = TempDir::new("ccproxy-state-project");
    let state_dir = TempDir::new("ccproxy-state-config");
    let config = routing_config(Vec::new());
    let state = test_state(config, &state_dir);
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);
    state
        .add_project_bindings("openai_responses", "m1", vec![path])
        .await
        .unwrap();
    assert!(project.join(".codex/config.toml").exists());
    let id = state.config_snapshot().settings.project_model_bindings[0]
        .id
        .clone();

    state
        .remove_project_binding("openai_responses", &id)
        .unwrap();
    assert!(state
        .config_snapshot()
        .settings
        .project_model_bindings
        .is_empty());
    assert!(project.join(".codex/config.toml").exists());
}

#[tokio::test]
async fn candidate_lists_are_filtered_by_protocol() {
    let codex_project = TempDir::new("ccproxy-candidate-codex");
    let claude_project = TempDir::new("ccproxy-candidate-claude");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(Vec::new()), &state_dir);
    let codex_path = std::fs::canonicalize(&codex_project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let claude_path = std::fs::canonicalize(&claude_project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![
        candidate(&codex_path, true, false),
        candidate(&claude_path, false, true),
    ]);

    let codex = state
        .project_models_view("openai_responses", "m1", false)
        .await
        .unwrap();
    let claude = state
        .project_models_view("anthropic", "m2", false)
        .await
        .unwrap();
    assert_eq!(codex.directory_candidates, vec![codex_path]);
    assert_eq!(claude.directory_candidates, vec![claude_path]);
    assert_eq!(codex.model, "codex-a");
    assert_eq!(claude.model, "claude-a");
}

#[tokio::test]
async fn add_rejects_a_candidate_from_the_other_client() {
    let project = TempDir::new("ccproxy-candidate-wrong-client");
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(Vec::new()), &state_dir);
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);

    let error = state
        .add_project_bindings("anthropic", "m2", vec![path])
        .await
        .unwrap_err();
    assert!(error.contains("不在当前客户端的会话候选"));
}

#[tokio::test]
async fn two_claude_candidates_cannot_share_a_settings_file() {
    let repo = TempDir::new("ccproxy-claude-candidates-repo");
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo.path)
        .status()
        .unwrap();
    let first = repo.join("first");
    let second = repo.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let first = std::fs::canonicalize(first)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let second = std::fs::canonicalize(second)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let state_dir = TempDir::new("ccproxy-state-config");
    let state = test_state(routing_config(Vec::new()), &state_dir);
    state.cache_project_candidates(vec![
        candidate(&first, false, true),
        candidate(&second, false, true),
    ]);

    state
        .add_project_bindings("anthropic", "m2", vec![first])
        .await
        .unwrap();
    let error = state
        .add_project_bindings("anthropic", "m2", vec![second])
        .await
        .unwrap_err();
    assert!(error.contains("共用 settings.local.json"));
}
