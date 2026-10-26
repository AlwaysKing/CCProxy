// 项目默认模型写入：只改目标 model，保留用户配置里的其余字节和结构。

use ccproxy_lib::clients::apply::rewrite_codex_model;
use ccproxy_lib::clients::json::set_object_values;
use serde_json::Value;

#[test]
fn claude_root_model_is_replaced_without_reformatting() {
    let before = "{\r\n  \"env\": { \"A\": 1 },\r\n  \"model\": \"old\"\r\n}";
    let next = set_object_values(
        before,
        "",
        &[("model".to_string(), Value::String("new".to_string()))],
    )
    .unwrap();
    assert_eq!(
        next.text,
        "{\r\n  \"env\": { \"A\": 1 },\r\n  \"model\": \"new\"\r\n}"
    );
}

#[test]
fn codex_model_update_preserves_comments_crlf_and_other_fields() {
    let before = "# keep\r\nmodel = \"old\" # old\r\n[features]\r\nmulti_agent = true\r\n";
    let after = rewrite_codex_model(before, "new").unwrap();
    assert_eq!(
        after,
        "# keep\r\nmodel = \"new\" # old\r\n[features]\r\nmulti_agent = true\r\n"
    );
}

#[test]
fn codex_model_update_handles_quotes_and_newlines() {
    let after = rewrite_codex_model("", "a\"b\nc\\d").unwrap();
    assert_eq!(after, "model = \"a\\\"b\\nc\\\\d\"\n");
    assert_eq!(
        after
            .parse::<toml_edit::DocumentMut>()
            .unwrap()
            .get("model")
            .and_then(|item| item.as_str()),
        Some("a\"b\nc\\d")
    );
}

#[test]
fn codex_model_update_supports_existing_multiline_values() {
    let before = "model = \"\"\"old\"\"\"\n[features]\nmulti_agent = true\n";
    let after = rewrite_codex_model(before, "new").unwrap();
    assert!(after.contains("model = \"new\""));
    assert!(after.contains("multi_agent = true"));
}
