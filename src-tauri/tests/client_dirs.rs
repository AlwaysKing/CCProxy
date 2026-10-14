// 客户端配置目录单测：身份判定（claude/codex/误判防护）、内置位置、扫描剔除与 kind 矫正

use serde_json::{json, Value};

use ccproxy_lib::clients::dirs::{builtin_dirs, detect_dir_kind, scan_client_dirs};
use ccproxy_lib::config::ClientDir;

use common::TempDir;

mod common;

fn dir_entry(id: &str, kind: &str, path: &str) -> ClientDir {
    ClientDir { id: id.to_string(), kind: kind.to_string(), path: path.to_string() }
}

#[test]
fn settings_json_marks_a_claude_dir() {
    let dir = TempDir::new("ccproxy-dirs");
    dir.write("settings.json", "{}");
    assert_eq!(detect_dir_kind(&dir.path_string()).as_deref(), Some("claude"));
}

#[test]
fn config_toml_with_codex_marker_marks_a_codex_dir() {
    for content in ["model_provider = \"custom\"\n", "[model_providers.custom]\nbase_url = \"http://x\"\n"] {
        let dir = TempDir::new("ccproxy-dirs");
        dir.write("config.toml", content);
        assert_eq!(detect_dir_kind(&dir.path_string()).as_deref(), Some("codex"));
    }
}

#[test]
fn config_toml_without_codex_marker_is_not_codex() {
    let dir = TempDir::new("ccproxy-dirs");
    dir.write("config.toml", "[build]\ntarget = \"x86_64-apple-darwin\"\n");
    assert_eq!(detect_dir_kind(&dir.path_string()), None);
}

#[test]
fn empty_missing_and_blank_paths_are_none_and_claude_wins_on_conflict() {
    let dir = TempDir::new("ccproxy-dirs");
    assert_eq!(detect_dir_kind(&dir.path_string()), None);
    assert_eq!(detect_dir_kind(&dir.join("no-such").to_string_lossy()), None);
    assert_eq!(detect_dir_kind(""), None);

    let both = TempDir::new("ccproxy-dirs");
    both.write("settings.json", "{}");
    both.write("config.toml", "model_provider = \"custom\"\n");
    assert_eq!(detect_dir_kind(&both.path_string()).as_deref(), Some("claude"));
}

#[test]
fn builtin_dirs_are_four_paths_with_two_kinds() {
    let home = std::path::PathBuf::from("/fake/home");
    let dirs = builtin_dirs(Some(&home));
    let actual: Value = serde_json::to_value(
        dirs.iter()
            .map(|entry| json!({ "kind": entry.kind, "path": entry.path }))
            .collect::<Vec<Value>>(),
    )
    .unwrap();
    assert_eq!(
        actual,
        json!([
            { "kind": "claude", "path": "/fake/home/.claude" },
            { "kind": "codex", "path": "/fake/home/.codex" },
            { "kind": "claude", "path": "/fake/home/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig" },
            { "kind": "codex", "path": "/fake/home/Library/Developer/Xcode/CodingAssistant/codex" }
        ])
    );
}

#[test]
fn scan_keeps_only_matching_builtins_and_fixes_manual_kinds() {
    let home = TempDir::new("ccproxy-home");
    // 只造一个有效的内置位置：~/.claude
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    home.write(".claude/settings.json", "{}");
    // ~/.codex 里放个没有 codex 特征键的 config.toml：路径在、身份不对，不该出现
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    home.write(".codex/config.toml", "[build]\n");

    let valid_claude = TempDir::new("ccproxy-dirs");
    valid_claude.write("settings.json", "{}");
    let gone_dir = TempDir::new("ccproxy-dirs");
    let gone = gone_dir.join("sub").to_string_lossy().to_string();
    let kind_changed = TempDir::new("ccproxy-dirs");
    kind_changed.write("settings.json", "{}");

    let manual = vec![
        dir_entry("d1", "claude", &valid_claude.path_string()),
        dir_entry("d2", "codex", &gone),
        dir_entry("d3", "codex", &kind_changed.path_string()),
    ];
    let scan = scan_client_dirs(Some(&home.path), &manual);

    let builtin: Value = serde_json::to_value(&scan.builtin).unwrap();
    assert_eq!(builtin, json!([{ "id": "", "kind": "claude", "path": home.join(".claude").to_string_lossy() }]));

    let kept: Value = serde_json::to_value(&scan.manual).unwrap();
    assert_eq!(
        kept,
        json!([
            { "id": "d1", "kind": "claude", "path": valid_claude.path_string() },
            { "id": "d3", "kind": "claude", "path": kind_changed.path_string() }
        ])
    );

    let dropped: Value = serde_json::to_value(&scan.dropped).unwrap();
    assert_eq!(dropped, json!([{ "id": "d2", "kind": "codex", "path": gone }]));
}

#[test]
fn scan_defaults_manual_to_empty() {
    let scan = scan_client_dirs(Some(std::path::Path::new("/fake/home")), &[]);
    assert!(scan.manual.is_empty());
    assert!(scan.dropped.is_empty());
}

