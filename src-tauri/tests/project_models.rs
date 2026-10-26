// 项目级默认模型：发现、候选校验和只写 model 的同步行为。

use serde_json::json;

use ccproxy_lib::clients::project_models::{
    apply_project_model, claude_settings_root, discover_projects, project_model_candidates,
    ProjectModelCandidate, ProjectSyncState,
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
    assert_eq!(
        candidates.codex,
        vec![ProjectModelCandidate {
            mapping_id: "m-codex".to_string(),
            model: "codex-alias".to_string(),
            invalid: false,
        }]
    );
    assert_eq!(
        candidates.claude,
        vec![ProjectModelCandidate {
            mapping_id: "m-claude".to_string(),
            model: "claude-alias".to_string(),
            invalid: false,
        }]
    );
}

#[test]
fn model_candidates_follow_first_match_routing() {
    let config = config_from(json!({
        "version": 6,
        "providers": [{
            "id": "p1",
            "name": "provider",
            "enabled": true,
            "baseUrl": "https://example.com",
            "token": "sk-test",
            "modelGroups": [{
                "id": "g1",
                "apiFormat": "anthropic",
                "models": [{ "name": "real" }]
            }]
        }],
        "protocols": [{
            "id": "anthropic",
            "name": "Anthropic",
            "enabled": true,
            "mappings": [
                {
                    "id": "wild",
                    "enabled": true,
                    "clientModel": "claude-3*",
                    "providerId": "p1",
                    "modelGroupId": "g1",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real"
                },
                {
                    "id": "shadowed",
                    "enabled": true,
                    "clientModel": "claude-3",
                    "providerId": "p1",
                    "modelGroupId": "g1",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real"
                },
                {
                    "id": "duplicate-ok",
                    "enabled": true,
                    "clientModel": "claude-free",
                    "providerId": "p1",
                    "modelGroupId": "g1",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real"
                },
                {
                    "id": "duplicate-shadowed",
                    "enabled": true,
                    "clientModel": "claude-free",
                    "providerId": "p1",
                    "modelGroupId": "g1",
                    "upstreamProtocol": "anthropic",
                    "realModel": "real"
                }
            ]
        }]
    }));

    let candidates = project_model_candidates(&config);
    assert_eq!(
        candidates
            .claude
            .iter()
            .map(|item| item.mapping_id.as_str())
            .collect::<Vec<_>>(),
        vec!["duplicate-ok"]
    );
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
fn claude_binding_preserves_existing_settings_and_leaves_a_backup() {
    let project = TempDir::new("ccproxy-project-claude-existing");
    let before =
        "{\n  \"permissions\": { \"allow\": [\"Bash(ls:*)\"] },\n  \"model\": \"old\"\n}\n";
    project.write(".claude/settings.local.json", before);
    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::Ok);
    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(written["model"], "claude-alias");
    assert_eq!(written["permissions"]["allow"][0], "Bash(ls:*)");
    assert_eq!(
        std::fs::read_to_string(project.join(".claude/settings.local.json.ccproxy-bak")).unwrap(),
        before
    );
}

#[test]
fn claude_binding_refuses_broken_json_without_touching_it() {
    let project = TempDir::new("ccproxy-project-claude-broken");
    project.write(".claude/settings.local.json", "{ broken");
    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::ConfigUnreadable);
    assert_eq!(
        std::fs::read_to_string(project.join(".claude/settings.local.json")).unwrap(),
        "{ broken"
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

#[test]
fn filesystem_root_cannot_be_used_as_a_project() {
    let result = apply_project_model(&routing_config(), "/", "codex", "codex-alias");
    assert_eq!(result.status, ProjectSyncState::BindingInvalid);
}

#[test]
fn claude_uses_the_git_root_for_a_nested_working_directory() {
    let repo = TempDir::new("ccproxy-project-repo");
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo.path)
        .status()
        .unwrap();
    let nested = repo.join("crates/app");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        std::fs::canonicalize(claude_settings_root(&nested.to_string_lossy())).unwrap(),
        std::fs::canonicalize(&repo.path).unwrap()
    );
    let result = apply_project_model(
        &routing_config(),
        &nested.to_string_lossy(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::Ok);
    assert!(repo.join(".claude/settings.local.json").exists());
    assert!(!nested.join(".claude/settings.local.json").exists());
}

#[test]
fn claude_worktree_uses_the_main_checkout_settings_file() {
    let container = TempDir::new("ccproxy-project-worktree-container");
    let repo = container.join("main");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "-c",
            "user.name=CCProxy Test",
            "-c",
            "user.email=ccproxy@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    let worktree = container.join("worktree");
    std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            worktree.to_string_lossy().as_ref(),
            "-b",
            "feature/worktree-test",
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    let nested = worktree.join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        std::fs::canonicalize(claude_settings_root(&nested.to_string_lossy())).unwrap(),
        std::fs::canonicalize(&repo).unwrap()
    );
    let result = apply_project_model(
        &routing_config(),
        &nested.to_string_lossy(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::Ok);
    assert!(repo.join(".claude/settings.local.json").exists());
    assert!(!worktree.join(".claude/settings.local.json").exists());
}

#[cfg(unix)]
#[test]
fn project_sync_refuses_a_symlinked_config_directory_outside_the_project() {
    let project = TempDir::new("ccproxy-project-symlink");
    let outside = TempDir::new("ccproxy-project-outside");
    outside.write("settings.local.json", "{\n  \"keep\": true\n}\n");
    std::os::unix::fs::symlink(&outside.path, project.join(".claude")).unwrap();

    let result = apply_project_model(
        &routing_config(),
        &project.path_string(),
        "claude",
        "claude-alias",
    );
    assert_eq!(result.status, ProjectSyncState::ConfigUnreadable);
    assert_eq!(
        std::fs::read_to_string(outside.join("settings.local.json")).unwrap(),
        "{\n  \"keep\": true\n}\n"
    );
}
