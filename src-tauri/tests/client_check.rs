// 客户端配置检查单测：claude 的 settings.json、codex 的 config.toml，
// 以及「指向本机但不是 CCProxy 端口」这种必须区分开的情况

use ccproxy_lib::clients::check::{inspect_client_config, InspectContext};
use ccproxy_lib::clients::catalog::ExpectedEntry;

use common::TempDir;

mod common;

const PORT: u16 = 18888;
const TOKEN: &str = "t_abc123";

fn context() -> InspectContext {
    InspectContext::new(PORT, TOKEN, Vec::<ExpectedEntry>::new())
}

fn claude_dir(env: serde_json::Value) -> TempDir {
    let dir = TempDir::new("ccproxy-check");
    let payload = serde_json::json!({ "env": env, "permissions": { "allow": ["Bash(ls:*)"] } });
    dir.write("settings.json", &serde_json::to_string_pretty(&payload).unwrap());
    dir
}

fn codex_dir(content: &str) -> TempDir {
    let dir = TempDir::new("ccproxy-check");
    dir.write("config.toml", content);
    dir
}

#[test]
fn address_must_be_loopback_with_the_current_port() {
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1:18888", 18888));
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("http://localhost:18888/v1", 18888));
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("http://[::1]:18888", 18888));
    // 本机但端口不是 CCProxy 的（别的本地代理就挂在别的端口上）
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1:15721", 18888));
    // 不写端口 = 默认 80/443，不可能等于我们的端口
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1", 18888));
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("http://192.168.60.2:18888", 18888));
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("127.0.0.1:18888", 18888));
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("", 18888));
}

#[test]
fn default_ports_are_restored() {
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1", 80));
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1:80", 80));
    assert!(ccproxy_lib::clients::check::is_ccproxy_address("https://localhost", 443));
    // 补默认端口不能把别的端口一起放进来
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("http://127.0.0.1:8080", 80));
    assert!(!ccproxy_lib::clients::check::is_ccproxy_address("https://localhost:80", 443));
}

#[test]
fn claude_ok_when_address_and_token_match() {
    let dir = claude_dir(serde_json::json!({
        "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888",
        "ANTHROPIC_AUTH_TOKEN": TOKEN
    }));
    let result = inspect_client_config(&dir.path_string(), "claude", &context());
    assert_eq!(result.status, "ok");
    assert_eq!(result.target, "http://127.0.0.1:18888");
    assert!(result.problems.is_empty());
}

#[test]
fn claude_accepts_base_url_with_v1() {
    let dir = claude_dir(serde_json::json!({
        "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888/v1",
        "ANTHROPIC_AUTH_TOKEN": TOKEN
    }));
    assert_eq!(inspect_client_config(&dir.path_string(), "claude", &context()).status, "ok");
}

#[test]
fn claude_pointing_at_another_local_port_is_mismatch() {
    let dir = claude_dir(serde_json::json!({
        "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
        "ANTHROPIC_AUTH_TOKEN": TOKEN
    }));
    let result = inspect_client_config(&dir.path_string(), "claude", &context());
    assert_eq!(result.status, "mismatch");
    assert_eq!(result.target, "http://127.0.0.1:15721");
    assert_eq!(
        result.problems,
        vec!["ANTHROPIC_BASE_URL 指向 http://127.0.0.1:15721，不是 CCProxy（http://127.0.0.1:18888）"]
    );
}

#[test]
fn claude_reports_bad_token_and_missing_values() {
    let bad_token = claude_dir(serde_json::json!({
        "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888",
        "ANTHROPIC_AUTH_TOKEN": "other"
    }));
    assert_eq!(
        inspect_client_config(&bad_token.path_string(), "claude", &context()).problems,
        vec!["ANTHROPIC_AUTH_TOKEN token 与 CCProxy 的客户端 token 不一致"]
    );

    let empty = claude_dir(serde_json::json!({}));
    assert_eq!(
        inspect_client_config(&empty.path_string(), "claude", &context()).problems,
        vec!["没有配置 ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN 没有配置 token"]
    );
}

#[test]
fn claude_unreadable_cases() {
    let missing = TempDir::new("ccproxy-check");
    assert_eq!(inspect_client_config(&missing.path_string(), "claude", &context()).status, "unreadable");

    let broken = TempDir::new("ccproxy-check");
    broken.write("settings.json", "{ 不是 json");
    let result = inspect_client_config(&broken.path_string(), "claude", &context());
    assert!(result.problems[0].contains("不是合法 JSON"));

    let array = TempDir::new("ccproxy-check");
    array.write("settings.json", "[1,2]");
    let result = inspect_client_config(&array.path_string(), "claude", &context());
    assert!(result.problems[0].contains("根节点不是对象"));
}

#[test]
fn codex_ok_when_the_selected_provider_block_matches() {
    let dir = codex_dir(&[
        "model_provider = \"custom\"",
        "model = \"ccproxy\"",
        "",
        "[model_providers.custom]",
        "name = \"custom\"",
        "wire_api = \"responses\"",
        "requires_openai_auth = false",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    let result = inspect_client_config(&dir.path_string(), "codex", &context());
    assert_eq!(result.status, "ok");
    assert_eq!(result.target, "http://127.0.0.1:18888/v1");
    assert!(result.problems.is_empty());
}

#[test]
fn codex_reads_the_selected_provider_block_only() {
    let dir = codex_dir(&[
        "model_provider = \"ccswitch\"",
        "",
        "[model_providers.other]",
        "name = \"codexcn\"",
        "base_url = \"https://api2.example.com/v1\"",
        "",
        "[model_providers.ccswitch]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    assert_eq!(inspect_client_config(&dir.path_string(), "codex", &context()).status, "ok");
}

#[test]
fn codex_section_names_with_comments_or_quotes_still_match() {
    let commented = codex_dir(&[
        "model_provider = \"ccproxy\"",
        "",
        "[model_providers.ccproxy] # CCProxy 写入的",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    assert_eq!(inspect_client_config(&commented.path_string(), "codex", &context()).status, "ok");

    let quoted = codex_dir(&[
        "model_provider = \"ccproxy\"",
        "",
        "[model_providers.\"ccproxy\"]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    assert_eq!(inspect_client_config(&quoted.path_string(), "codex", &context()).status, "ok");
}

#[test]
fn codex_unterminated_multiline_string_is_unreadable() {
    let dir = codex_dir("model_provider = \"custom\"\ndev = \"\"\"\n这里没闭合\n");
    let result = inspect_client_config(&dir.path_string(), "codex", &context());
    assert_eq!(result.status, "unreadable");
    assert!(result.problems[0].contains("多行字符串没闭合"));
}

#[test]
fn codex_missing_or_ghost_provider_is_unreadable() {
    let no_provider = codex_dir("model = \"ccproxy\"\n[features]\nmulti_agent = true\n");
    let result = inspect_client_config(&no_provider.path_string(), "codex", &context());
    assert!(result.problems[0].contains("没有配置 model_provider"));

    let ghost = codex_dir("model_provider = \"ghost\"\n\n[model_providers.other]\nbase_url = \"http://127.0.0.1:18888\"\n");
    let result = inspect_client_config(&ghost.path_string(), "codex", &context());
    assert!(result.problems[0].contains("没有 [model_providers.ghost] 块"), "实际：{:?}", result.problems);
}

#[test]
fn codex_requires_openai_auth_is_reported_separately() {
    let dir = codex_dir(&[
        "model_provider = \"custom\"",
        "",
        "[model_providers.custom]",
        "requires_openai_auth = true",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    let result = inspect_client_config(&dir.path_string(), "codex", &context());
    assert_eq!(result.status, "mismatch");
    // 地址是对的，但凭据不走这个 token，不能算通过；也不该再多报一条「token 不一致」
    assert_eq!(
        result.problems,
        vec!["requires_openai_auth 为 true，codex 会用 auth.json 里的凭据而不是这个 token"]
    );
}

#[test]
fn codex_multiline_blocks_hide_their_content() {
    for delimiter in ["\"\"\"", "'''"] {
        let dir = codex_dir(&[
            "model_provider = \"custom\"",
            &format!("developer_instructions = {delimiter}"),
            "model_provider = \"hijacked\"",
            delimiter,
            "",
            "[model_providers.custom]",
            "base_url = \"http://127.0.0.1:18888/v1\"",
            "experimental_bearer_token = \"t_abc123\"",
        ]
        .join("\n"));
        assert_eq!(
            inspect_client_config(&dir.path_string(), "codex", &context()).status,
            "ok",
            "分隔符 {delimiter} 的块没被隔离"
        );
    }
}

#[test]
fn codex_comments_and_trailing_comments_do_not_interfere() {
    let dir = codex_dir(&[
        "# model_provider = \"commented\"",
        "model_provider = \"custom\"",
        "model_reasoning_effort = \"medium\"",
        "",
        "# [model_providers.other]",
        "[model_providers.custom]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
        "experimental_bearer_token = \"t_abc123\"",
    ]
    .join("\n"));
    assert_eq!(inspect_client_config(&dir.path_string(), "codex", &context()).status, "ok");

    let trailing = codex_dir(&[
        "model_provider = \"custom\" # 走本地代理",
        "",
        "[model_providers.custom]",
        "base_url = \"http://127.0.0.1:18888/v1\" # 本地代理",
        "experimental_bearer_token = \"t_abc123\" # 填 CCProxy 的",
    ]
    .join("\n"));
    let result = inspect_client_config(&trailing.path_string(), "codex", &context());
    assert_eq!(result.status, "ok");
    assert_eq!(result.target, "http://127.0.0.1:18888/v1");
}

#[test]
fn unknown_dir_kind_is_reported() {
    let dir = TempDir::new("ccproxy-check");
    let result = inspect_client_config(&dir.path_string(), "gemini", &context());
    assert_eq!(result.status, "unreadable");
    assert!(result.problems[0].contains("未知的目录类型"));
}

