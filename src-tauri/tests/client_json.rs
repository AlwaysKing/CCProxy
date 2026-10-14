// JSON 定点改写单测：只换目标值的字节区间，缩进、键序、转义、行尾一个字节都不动

use serde_json::{json, Value};

use ccproxy_lib::clients::json::set_object_values;

fn values() -> Vec<(String, Value)> {
    vec![
        ("ANTHROPIC_BASE_URL".to_string(), json!("http://127.0.0.1:18888")),
        ("ANTHROPIC_AUTH_TOKEN".to_string(), json!("t_abc123")),
    ]
}

#[test]
fn only_replaces_the_two_target_values() {
    let before = [
        "{",
        "  \"hooks\": { \"PreToolUse\": [ { \"matcher\": \"Bash\", \"hooks\": [ { \"type\": \"command\" } ] } ] },",
        "  \"env\": {",
        "    \"ANTHROPIC_BASE_URL\": \"http://192.168.60.2:9005\",",
        "    \"ANTHROPIC_AUTH_TOKEN\": \"sk-old\",",
        "    \"NOTE\": \"env 里写着 { \\\"brace\\\": 1 } 也不算数\"",
        "  }",
        "}",
    ]
    .join("\n");

    let result = set_object_values(&before, "env", &values()).unwrap();
    assert!(result.changed);
    assert_eq!(
        result.text,
        before
            .replace("\"http://192.168.60.2:9005\"", "\"http://127.0.0.1:18888\"")
            .replace("\"sk-old\"", "\"t_abc123\"")
    );
}

#[test]
fn already_correct_values_are_left_untouched() {
    let before = "{\"env\":{\"ANTHROPIC_BASE_URL\":\"http://127.0.0.1:18888\",\"ANTHROPIC_AUTH_TOKEN\":\"other\"}}";
    let result = set_object_values(before, "env", &values()).unwrap();
    assert_eq!(
        result.text,
        "{\"env\":{\"ANTHROPIC_BASE_URL\":\"http://127.0.0.1:18888\",\"ANTHROPIC_AUTH_TOKEN\":\"t_abc123\"}}"
    );

    let done = set_object_values(&result.text, "env", &values()).unwrap();
    assert!(!done.changed);
    assert_eq!(done.text, result.text);
}

#[test]
fn missing_keys_follow_the_file_indent_unit() {
    let before = [
        "{",
        "    \"env\": {",
        "        \"ANTHROPIC_BASE_URL\": \"http://192.168.60.2:9005\"",
        "    }",
        "}",
    ]
    .join("\n");

    let result = set_object_values(&before, "env", &values()).unwrap();
    assert_eq!(
        result.text,
        [
            "{",
            "    \"env\": {",
            "        \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:18888\",",
            "        \"ANTHROPIC_AUTH_TOKEN\": \"t_abc123\"",
            "    }",
            "}",
        ]
        .join("\n")
    );
}

#[test]
fn missing_env_block_is_appended_after_the_last_member() {
    let before = ["{", "  \"model\": \"x\",", "  \"permissions\": { \"allow\": [] }", "}"].join("\n");
    let result = set_object_values(&before, "env", &values()).unwrap();
    assert_eq!(
        result.text,
        [
            "{",
            "  \"model\": \"x\",",
            "  \"permissions\": { \"allow\": [] },",
            "  \"env\": {",
            "    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:18888\",",
            "    \"ANTHROPIC_AUTH_TOKEN\": \"t_abc123\"",
            "  }",
            "}",
        ]
        .join("\n")
    );
}

#[test]
fn empty_root_and_non_object_env_both_get_rebuilt() {
    let empty_root = set_object_values("{}\n", "env", &values()).unwrap();
    assert_eq!(
        empty_root.text,
        [
            "{",
            "  \"env\": {",
            "    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:18888\",",
            "    \"ANTHROPIC_AUTH_TOKEN\": \"t_abc123\"",
            "  }",
            "}",
            "",
        ]
        .join("\n")
    );

    // env 是字符串：这一块归我们写，原地换成一个对象
    let broken = set_object_values("{\"env\":\"oops\"}", "env", &values()).unwrap();
    let parsed: Value = serde_json::from_str(&broken.text).unwrap();
    assert_eq!(parsed["env"]["ANTHROPIC_AUTH_TOKEN"], json!("t_abc123"));
    assert!(!broken.text.contains("oops"));
}

#[test]
fn escaped_key_names_are_decoded() {
    let before = "{\"env\":{\"ANTHROPIC\\u005fBASE_URL\":\"http://192.168.60.2:9005\"}}";
    let result = set_object_values(before, "env", &values()).unwrap();
    assert!(result.text.contains("\"http://127.0.0.1:18888\""));
    let parsed: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(parsed["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
}

#[test]
fn single_line_json_stays_valid_and_other_keys_untouched() {
    let before = "{\"model\":\"x\",\"env\":{\"ANTHROPIC_AUTH_TOKEN\":\"sk-old\"},\"z\":[1,2,3]}";
    let result = set_object_values(before, "env", &values()).unwrap();
    let parsed: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(parsed["model"], json!("x"));
    assert_eq!(parsed["z"], json!([1, 2, 3]));
    assert_eq!(parsed["env"]["ANTHROPIC_AUTH_TOKEN"], json!("t_abc123"));
    assert_eq!(parsed["env"]["ANTHROPIC_BASE_URL"], json!("http://127.0.0.1:18888"));
}

#[test]
fn broken_json_or_non_object_root_is_refused() {
    let error = set_object_values("{ 这不是 json", "env", &values()).unwrap_err();
    assert!(error.contains("不是合法 JSON，为免写坏已放弃"), "实际：{error}");
    let error = set_object_values("[1,2]", "env", &values()).unwrap_err();
    assert!(error.contains("根节点不是对象，为免写坏已放弃"), "实际：{error}");
}

