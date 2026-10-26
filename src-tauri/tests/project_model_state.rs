// 项目模型状态层：统一项目视图、按客户端设置模型和映射变化同步。

use serde_json::json;

use ccproxy_lib::clients::project_models::{DiscoveredProject, ProjectSources};
use ccproxy_lib::config::AppConfig;

use common::{config_from, test_state, TempDir};

mod common;

fn routing_config(bindings: Vec<serde_json::Value>) -> AppConfig {
    config_from(json!({
        "version": 6,
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
async fn unified_view_merges_sources_and_sorts_paths() {
    let root = TempDir::new("ccproxy-unified-view");
    let z_path = root.join("z-project");
    let a_path = root.join("a-project");
    let offline = root.join("m-project");
    std::fs::create_dir_all(&z_path).unwrap();
    std::fs::create_dir_all(&a_path).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    let z_path = std::fs::canonicalize(z_path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let a_path = std::fs::canonicalize(a_path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let offline_path = std::fs::canonicalize(&offline)
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::remove_dir_all(&offline).unwrap();
    let config = routing_config(vec![json!({
        "id": "b-offline",
        "path": offline_path,
        "client": "codex",
        "mappingId": "m1"
    })]);
    let state = test_state(config, &TempDir::new("ccproxy-unified-view-config"));
    state.cache_project_candidates(vec![
        candidate(&z_path, true, true),
        candidate(&a_path, false, true),
    ]);

    let view = state.unified_project_models_view(false).await;

    assert_eq!(view.projects.len(), 3);
    assert!(view
        .projects
        .windows(2)
        .all(|pair| pair[0].path <= pair[1].path));
    let merged = view.projects.iter().find(|row| row.path == z_path).unwrap();
    assert!(merged.sources.codex && merged.sources.claude);
    let offline = view
        .projects
        .iter()
        .find(|row| row.path == offline_path)
        .unwrap();
    assert!(offline.missing);
    assert_eq!(offline.codex_mapping_id.as_deref(), Some("m1"));
    assert_eq!(offline.codex_model.as_deref(), Some("codex-a"));
}

#[tokio::test]
async fn set_model_preserves_binding_id_and_clear_keeps_client_file() {
    let project = TempDir::new("ccproxy-unified-set");
    let state = test_state(
        routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-set-config"),
    );
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, true)]);

    state
        .set_project_model(&path, "codex", Some("m1"))
        .await
        .unwrap();
    let id = state.config_snapshot().settings.project_model_bindings[0]
        .id
        .clone();
    state
        .set_project_model(&path, "codex", Some("m1"))
        .await
        .unwrap();
    assert_eq!(
        state.config_snapshot().settings.project_model_bindings[0].id,
        id
    );

    state.set_project_model(&path, "codex", None).await.unwrap();
    assert!(state
        .config_snapshot()
        .settings
        .project_model_bindings
        .is_empty());
    assert_eq!(
        std::fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        "model = \"codex-a\"\n"
    );
}

#[tokio::test]
async fn clearing_one_client_keeps_the_other_binding() {
    let project = TempDir::new("ccproxy-unified-two-clients");
    let state = test_state(
        routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-two-clients-config"),
    );
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, true)]);

    state
        .set_project_model(&path, "codex", Some("m1"))
        .await
        .unwrap();
    state
        .set_project_model(&path, "claude", Some("m2"))
        .await
        .unwrap();
    state.set_project_model(&path, "codex", None).await.unwrap();

    let bindings = state.config_snapshot().settings.project_model_bindings;
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].client, "claude");
    assert!(project.join(".codex/config.toml").exists());
    assert!(project.join(".claude/settings.local.json").exists());
}

#[tokio::test]
async fn offline_existing_binding_can_be_cleared() {
    let project = TempDir::new("ccproxy-unified-offline-clear");
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let state = test_state(
        routing_config(vec![json!({
            "id": "b1",
            "path": path,
            "client": "codex",
            "mappingId": "m1"
        })]),
        &TempDir::new("ccproxy-unified-offline-clear-config"),
    );
    state.cache_project_candidates(vec![candidate(&path, true, false)]);
    std::fs::remove_dir_all(&project.path).unwrap();

    state.set_project_model(&path, "codex", None).await.unwrap();
    assert!(state
        .config_snapshot()
        .settings
        .project_model_bindings
        .is_empty());
}

#[tokio::test]
async fn stale_cached_directory_cannot_create_a_new_binding() {
    let project = TempDir::new("ccproxy-unified-stale");
    let state = test_state(
        routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-stale-config"),
    );
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);
    std::fs::remove_dir_all(&project.path).unwrap();

    let error = state
        .set_project_model(&path, "codex", Some("m1"))
        .await
        .unwrap_err();
    assert!(error.contains("项目目录不存在"));
    assert!(state
        .config_snapshot()
        .settings
        .project_model_bindings
        .is_empty());
}

#[tokio::test]
async fn unknown_mapping_does_not_change_config() {
    let project = TempDir::new("ccproxy-unified-unknown");
    let state = test_state(
        routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-unknown-config"),
    );
    let path = std::fs::canonicalize(&project.path)
        .unwrap()
        .to_string_lossy()
        .to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);

    let error = state
        .set_project_model(&path, "codex", Some("missing"))
        .await
        .unwrap_err();
    assert!(error.contains("不可作为项目默认模型"));
    assert!(state
        .config_snapshot()
        .settings
        .project_model_bindings
        .is_empty());
}

#[tokio::test]
async fn two_claude_projects_cannot_share_a_settings_file() {
    let repo = TempDir::new("ccproxy-unified-claude-conflict");
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
    let state = test_state(
        routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-claude-conflict-config"),
    );
    state.cache_project_candidates(vec![
        candidate(&first, false, true),
        candidate(&second, false, true),
    ]);

    state
        .set_project_model(&first, "claude", Some("m2"))
        .await
        .unwrap();
    let error = state
        .set_project_model(&second, "claude", Some("m2"))
        .await
        .unwrap_err();
    assert!(
        error.contains("使用同一个 Claude settings.local.json"),
        "{error}"
    );
}
