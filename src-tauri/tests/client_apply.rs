// 写入客户端配置单测：claude 的 settings.json、codex 的 config.toml 改写，
// 以及「只动该动的、其余原样」这条底线（用户文件里别的东西一个都不能少）

use serde_json::{json, Value};

use ccproxy_lib::clients::apply::{apply_client_config, rewrite_codex_config};
use ccproxy_lib::clients::catalog::ExpectedEntry;
use ccproxy_lib::clients::check::{inspect_client_config, InspectContext};

use common::TempDir;

mod common;

const PORT: u16 = 18888;
const TOKEN: &str = "t_abc123";

fn context() -> InspectContext {
    InspectContext::new(PORT, TOKEN, Vec::<ExpectedEntry>::new())
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn claude_writes_env_keys_and_leaves_everything_else() {
    let dir = TempDir::new("ccproxy-apply");
    let original = json!({
        "permissions": { "allow": ["Bash(ls:*)"] },
        "model": "claude-sonnet-5",
        "enabledPlugins": { "x@y": true },
        "env": {
            "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005",
            "ANTHROPIC_AUTH_TOKEN": "sk-old",
            "ANTHROPIC_MODEL": "claude-sonnet-5",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.3"
        }
    });
    dir.write("settings.json", &serde_json::to_string_pretty(&original).unwrap());

    let results = apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    let written: Value = serde_json::from_str(&read(std::path::Path::new(&results[0].file))).unwrap();
    assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
    assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], json!("t_abc123"));
    // 模型名不碰（模型映射是另一件事），权限与插件原样
    assert_eq!(written["env"]["ANTHROPIC_MODEL"], json!("claude-sonnet-5"));
    assert_eq!(written["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("glm-5.3"));
    assert_eq!(written["permissions"], original["permissions"]);
    assert_eq!(written["enabledPlugins"], original["enabledPlugins"]);
    assert_eq!(written["model"], json!("claude-sonnet-5"));
}

#[test]
fn claude_write_makes_check_pass_and_leaves_a_backup() {
    let dir = TempDir::new("ccproxy-apply");
    let original = serde_json::to_string(&json!({ "env": { "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005" } })).unwrap();
    let file_path = dir.write("settings.json", &original);

    let results = apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    let backup = results[0].backup_path.clone().expect("应有备份");
    assert_eq!(backup, format!("{}.ccproxy-bak", file_path.to_string_lossy()));
    assert_eq!(read(std::path::Path::new(&backup)), original);
    assert_eq!(inspect_client_config(&dir.path_string(), "claude", &context()).status, "ok");
}

#[test]
fn claude_only_replaces_the_two_value_byte_ranges() {
    let dir = TempDir::new("ccproxy-apply");
    // 4 空格缩进、CRLF 行尾、\u 转义、末尾没有换行——全是「不该被我们改掉」的字节
    let before = [
        "{",
        "    \"permissions\": {",
        "        \"allow\": [ \"Bash(ls:*)\" ]",
        "    },",
        "    \"env\": {",
        "        \"ANTHROPIC_AUTH_TOKEN\": \"sk-old\",",
        "        \"ANTHROPIC_MODEL\": \"claude-\\u00e9\",",
        "        \"ANTHROPIC_BASE_URL\": \"http://192.168.60.2:9005\"",
        "    }",
        "}",
    ]
    .join("\r\n");
    dir.write("settings.json", &before);

    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    let after = read(&dir.join("settings.json"));
    assert_eq!(
        after,
        before
            .replace("\"http://192.168.60.2:9005\"", "\"http://127.0.0.1:18888\"")
            .replace("\"sk-old\"", "\"t_abc123\"")
    );
    // 转义没被解码、行尾没被换、末尾也没被补换行
    assert!(after.contains("\"claude-\\u00e9\""));
    assert!(after.contains("\r\n"));
    assert!(!after.ends_with('\n'));
}

#[test]
fn claude_changes_only_the_wrong_line() {
    let dir = TempDir::new("ccproxy-apply");
    let before = "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://192.168.60.2:9005\",\n    \"ANTHROPIC_AUTH_TOKEN\": \"t_abc123\"\n  }\n}\n";
    dir.write("settings.json", before);
    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    assert_eq!(
        read(&dir.join("settings.json")),
        before.replace("\"http://192.168.60.2:9005\"", "\"http://127.0.0.1:18888\"")
    );
}

#[test]
fn claude_does_not_touch_files_that_are_already_correct() {
    let dir = TempDir::new("ccproxy-apply");
    let before = serde_json::to_string_pretty(&json!({
        "env": { "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888", "ANTHROPIC_AUTH_TOKEN": TOKEN }
    }))
    .unwrap();
    let file_path = dir.write("settings.json", &before);

    let results = apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    assert!(results[0].backup_path.is_none());
    assert!(!results[0].changed);
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
    assert_eq!(read(&file_path), before);
}

#[test]
fn claude_creates_missing_file_and_env_block() {
    let empty = TempDir::new("ccproxy-apply");
    apply_client_config(&empty.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    assert_eq!(inspect_client_config(&empty.path_string(), "claude", &context()).status, "ok");

    let no_env = TempDir::new("ccproxy-apply");
    no_env.write("settings.json", &serde_json::to_string(&json!({ "model": "x" })).unwrap());
    apply_client_config(&no_env.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    let written: Value = serde_json::from_str(&read(&no_env.join("settings.json"))).unwrap();
    assert_eq!(written["model"], json!("x"));
    assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
}

#[test]
fn claude_refuses_to_touch_broken_settings_json() {
    let dir = TempDir::new("ccproxy-apply");
    let file_path = dir.write("settings.json", "{ 这不是 json");
    let error = apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap_err();
    assert!(error.contains("为免写坏已放弃"), "实际：{error}");
    // 原文件与备份都不该被创建/改动
    assert_eq!(read(&file_path), "{ 这不是 json");
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
}

#[test]
fn claude_fills_empty_env_object() {
    let dir = TempDir::new("ccproxy-apply");
    dir.write("settings.json", "{\n  \"env\": {}\n}\n");
    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    let written: Value = serde_json::from_str(&read(&dir.join("settings.json"))).unwrap();
    assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
    assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], json!("t_abc123"));
    assert_eq!(inspect_client_config(&dir.path_string(), "claude", &context()).status, "ok");
}

#[cfg(unix)]
#[test]
fn claude_write_is_atomic_and_keeps_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("ccproxy-apply");
    let file_path = dir.write("settings.json", "{}");
    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o640)).unwrap();
    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();

    let mut names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["settings.json".to_string(), "settings.json.ccproxy-bak".to_string()]);
    assert_eq!(std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777, 0o640);
}

#[cfg(unix)]
#[test]
fn claude_keeps_full_mode_including_group_write() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("ccproxy-apply");
    let file_path = dir.write("settings.json", "{}");
    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o666)).unwrap();
    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    assert_eq!(std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777, 0o666);
}

#[cfg(unix)]
#[test]
fn claude_follows_symlink_and_keeps_the_link() {
    let dir = TempDir::new("ccproxy-apply");
    let real = TempDir::new("ccproxy-apply-real");
    // dotfiles 仓库那类用法：~/.claude/settings.json 软链到别处
    let real_path = real.write(
        "settings.json",
        &serde_json::to_string(&json!({ "env": { "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005" } })).unwrap(),
    );
    let link_path = dir.join("settings.json");
    std::os::unix::fs::symlink(&real_path, &link_path).unwrap();

    apply_client_config(&dir.path_string(), "claude", PORT, TOKEN, &[]).unwrap();
    assert!(std::fs::symlink_metadata(&link_path).unwrap().file_type().is_symlink());
    let written: Value = serde_json::from_str(&read(&real_path)).unwrap();
    assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
    // 备份落在真实文件旁边，链接目录里不留东西
    assert!(std::path::PathBuf::from(format!("{}.ccproxy-bak", real_path.to_string_lossy())).exists());
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
    assert_eq!(inspect_client_config(&dir.path_string(), "claude", &context()).status, "ok");
}

// ============================================================================ codex

#[test]
fn codex_points_model_provider_at_ccproxy_and_rewrites_our_block() {
    let dir = TempDir::new("ccproxy-apply");
    dir.write(
        "config.toml",
        &[
            "model_provider = \"ccswitch\"",
            "model = \"gpt-5.6-sol\"",
            "model_reasoning_effort = \"medium\"",
            "",
            "[model_providers.ccswitch]",
            "name = \"ccswitch\"",
            "base_url = \"http://192.168.60.2:9005/v1\"",
            "wire_api = \"responses\"",
            "experimental_bearer_token = \"sk-old\"",
            "",
            "[features]",
            "multi_agent = true",
            "",
        ]
        .join("\n"),
    );

    let results = apply_client_config(&dir.path_string(), "codex", PORT, TOKEN, &[]).unwrap();
    let text = read(std::path::Path::new(&results[0].file));
    assert!(text.contains("model_provider = \"ccproxy\""));
    assert!(text.contains("[model_providers.ccproxy]"));
    assert!(text.contains(&format!("base_url = \"http://127.0.0.1:{PORT}/v1\"")));
    assert!(text.contains("requires_openai_auth = false"));
    assert!(text.contains("experimental_bearer_token = \"t_abc123\""));
    // 用户自己的东西一个不少
    assert!(text.contains("model = \"gpt-5.6-sol\""));
    assert!(text.contains("model_reasoning_effort = \"medium\""));
    assert!(text.contains("[model_providers.ccswitch]"));
    assert!(text.contains("[features]"));
    assert!(text.contains("multi_agent = true"));
    // 旧块里的键不留第二套
    assert_eq!(text.matches("[model_providers.ccproxy]").count(), 1);
    assert!(inspect_client_config(&dir.path_string(), "codex", &context()).status == "ok");
}

#[test]
fn codex_rewrite_preserves_comments_indent_crlf_and_trailing_newline() {
    let before = [
        "# 我的 codex 配置",
        "model_provider = \"custom\" # 走中转",
        "model = \"gpt-5.6-sol\"",
        "",
        "[model_providers.custom]",
        "base_url = \"https://api.example.com/v1\"",
    ]
    .join("\r\n");
    let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
    assert!(
        after.starts_with(
            "# 我的 codex 配置\r\nmodel_provider = \"ccproxy\" # 走中转\r\nmodel = \"gpt-5.6-sol\"\r\n\r\n[model_providers.custom]\r\n"
        ),
        "注释与 CRLF 被动了：{after}"
    );
    // 用户的块之后空一行接我们的块，行尾跟文件一致
    assert!(after.contains("\r\n\r\n[model_providers.ccproxy]\r\nname = \"ccproxy\"\r\n"));
    assert!(after.ends_with(&format!("experimental_bearer_token = \"{TOKEN}\"\r\n")));
}

#[test]
fn codex_rewrite_replaces_our_block_without_leaving_two_copies() {
    let before = [
        "model_provider = \"ccproxy\"",
        "",
        "[model_providers.ccproxy]",
        "# 旧的说明",
        "name = \"old\"",
        "base_url = \"http://192.168.60.2:9005/v1\"",
        "experimental_bearer_token = \"sk-old\"",
        "wire_api = \"responses\"",
        "",
        "[features]",
        "multi_agent = true",
    ]
    .join("\n");
    let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
    assert_eq!(after.matches("[model_providers.ccproxy]").count(), 1);
    assert!(!after.contains("sk-old"));
    assert!(!after.contains("旧的说明"));
    assert!(!after.contains("name = \"old\""));
    assert!(after.contains("[features]\nmulti_agent = true"));
}

#[test]
fn codex_rewrite_recognizes_commented_or_quoted_same_name_block() {
    // 两个 [model_providers.ccproxy] 在 TOML 里是非法配置，codex 会直接读不动
    for header in ["[model_providers.ccproxy] # CCProxy 写入的", "[model_providers.\"ccproxy\"]"] {
        let before = [
            "model_provider = \"ccproxy\"",
            "",
            header,
            "name = \"old\"",
            "base_url = \"http://192.168.60.2:9005/v1\"",
            "experimental_bearer_token = \"sk-old\"",
        ]
        .join("\n");
        let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
        assert_eq!(
            after.matches("[model_providers.ccproxy]").count()
                + after.matches("[model_providers.\"ccproxy\"]").count(),
            1,
            "{header} 之后出现了两个同名表"
        );
        assert_eq!(after.matches("experimental_bearer_token = ").count(), 1, "新旧两套键值都留下了");
        assert!(!after.contains("sk-old"));
    }
}

#[test]
fn codex_rewrite_keeps_sections_after_our_block() {
    let before = [
        "model_provider = \"ccproxy\"",
        "",
        "[model_providers.ccproxy]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
        "",
        "[model_providers.backup] # 备用供应商",
        "# 我自己的，别动",
        "base_url = \"https://api.example.com/v1\"",
        "experimental_bearer_token = \"sk-secret\"",
    ]
    .join("\n");
    let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
    // 带行尾注释的节名认不出来时，它连同下面整段会被当成「我们块的内容」删掉
    assert!(after.contains(
        "[model_providers.backup] # 备用供应商\n# 我自己的，别动\nbase_url = \"https://api.example.com/v1\"\nexperimental_bearer_token = \"sk-secret\""
    ));
}

#[test]
fn codex_rewrite_leaves_the_line_alone_when_already_pointing_at_us() {
    let before = [
        "model_provider = 'ccproxy' # 写法是用户自己的",
        "",
        "[model_providers.ccproxy]",
        "name = \"ccproxy\"",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "wire_api = \"responses\"",
        "requires_openai_auth = false",
        "experimental_bearer_token = \"t_abc123\"",
        "",
    ]
    .join("\n");
    let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
    // 一个字节都没改
    assert_eq!(after, before);
}

#[test]
fn codex_rewrite_keeps_blank_lines_inside_multiline_strings() {
    let instructions = ["developer_instructions = \"\"\"", "第一条", "", "", "第三条", "\"\"\""].join("\n");
    let before = [
        "model_provider = \"custom\"",
        &instructions,
        "",
        "[model_providers.custom]",
        "base_url = \"http://x/v1\"",
    ]
    .join("\n");
    let after = rewrite_codex_config(&before, PORT, TOKEN, None).unwrap();
    assert!(after.contains("第一条\n\n\n第三条"), "多行字符串里的连续空行被压掉了");
}

#[test]
fn codex_rewrite_builds_a_usable_config_from_nothing() {
    let after = rewrite_codex_config("", PORT, TOKEN, None).unwrap();
    assert!(after.starts_with("model_provider = \"ccproxy\"\n\n[model_providers.ccproxy]\n"), "实际：{after}");

    // 只有顶层键、一个节都没有的文件也能补出 model_provider
    let after = rewrite_codex_config("model = \"gpt-5.6-sol\"\n", PORT, TOKEN, None).unwrap();
    assert!(after.starts_with("model_provider = \"ccproxy\"\nmodel = \"gpt-5.6-sol\"\n\n[model_providers.ccproxy]\n"));
}

#[test]
fn codex_apply_keeps_the_users_own_provider_block() {
    let dir = TempDir::new("ccproxy-apply");
    dir.write(
        "config.toml",
        &[
            "model_provider = \"custom\"",
            "",
            "[model_providers.custom]",
            "base_url = \"http://127.0.0.1:15721/v1\"",
            "requires_openai_auth = true",
        ]
        .join("\n"),
    );
    apply_client_config(&dir.path_string(), "codex", PORT, TOKEN, &[]).unwrap();
    let text = read(&dir.join("config.toml"));
    // 用户自己那个块原样留着（它只是变成没人引用的死配置，删它才是越界）
    assert!(text.contains("[model_providers.custom]\nbase_url = \"http://127.0.0.1:15721/v1\"\nrequires_openai_auth = true"));
    // 指向我们的那个块必须显式 false
    assert!(text.contains(&format!(
        "[model_providers.ccproxy]\nname = \"ccproxy\"\nbase_url = \"http://127.0.0.1:{PORT}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n"
    )));
}

#[test]
fn codex_apply_fixes_a_dangling_model_provider() {
    let dir = TempDir::new("ccproxy-apply");
    dir.write("config.toml", "model_provider = \"ghost\"\n\n[model_providers.other]\nbase_url = \"http://x/v1\"\n");
    assert_eq!(inspect_client_config(&dir.path_string(), "codex", &context()).status, "unreadable");
    apply_client_config(&dir.path_string(), "codex", PORT, TOKEN, &[]).unwrap();
    assert_eq!(inspect_client_config(&dir.path_string(), "codex", &context()).status, "ok");
}

#[test]
fn codex_apply_leaves_a_backup() {
    let dir = TempDir::new("ccproxy-apply");
    let file_path = dir.write("config.toml", "model_provider = \"custom\"\n");
    let results = apply_client_config(&dir.path_string(), "codex", PORT, TOKEN, &[]).unwrap();
    let backup = results[0].backup_path.clone().unwrap();
    assert_eq!(backup, format!("{}.ccproxy-bak", file_path.to_string_lossy()));
    assert_eq!(read(std::path::Path::new(&backup)), "model_provider = \"custom\"\n");
    let mut names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["config.toml".to_string(), "config.toml.ccproxy-bak".to_string()]);
}

#[test]
fn codex_apply_refuses_unterminated_multiline_without_touching_anything() {
    let dir = TempDir::new("ccproxy-apply");
    let original = "model_provider = \"custom\"\ndev = \"\"\"\n这里没闭合\n";
    dir.write("config.toml", original);
    let error = apply_client_config(&dir.path_string(), "codex", PORT, TOKEN, &[]).unwrap_err();
    assert!(error.contains("为免写坏已放弃"), "实际：{error}");
    assert_eq!(read(&dir.join("config.toml")), original);
    let names: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["config.toml".to_string()]);
}

#[test]
fn codex_rewrite_adds_catalog_key_when_asked() {
    let text = rewrite_codex_config(
        "model_provider = \"old\"\n",
        PORT,
        TOKEN,
        Some("/Users/x/.codex/models.json"),
    )
    .unwrap();
    assert!(text.contains("model_catalog_json = \"/Users/x/.codex/models.json\""));
    // 顶层键必须写在第一个节之前
    let catalog_index = text.find("model_catalog_json").unwrap();
    let section_index = text.find("[model_providers.ccproxy]").unwrap();
    assert!(catalog_index < section_index);
}

#[test]
fn codex_rewrite_refuses_unterminated_multiline_strings() {
    let error = rewrite_codex_config("dev = \"\"\"\n没闭合\n", PORT, TOKEN, None).unwrap_err();
    assert!(error.contains("多行字符串没闭合"));
}

#[test]
fn apply_client_config_refuses_empty_token() {
    let dir = TempDir::new("ccproxy-apply");
    let error = apply_client_config(&dir.path_string(), "claude", PORT, "", &[]).unwrap_err();
    assert!(error.contains("还没有客户端 token"));
}

#[test]
fn apply_client_config_rejects_unknown_kind() {
    let dir = TempDir::new("ccproxy-apply");
    let error = apply_client_config(&dir.path_string(), "gemini", PORT, TOKEN, &[]).unwrap_err();
    assert!(error.contains("未知的目录类型"));
}

