// TOML 行扫描单测：行尾注释、引号内的 #、两种三引号多行字符串、带点的节名

use ccproxy_lib::clients::toml::{parse_toml_value, scan_toml_lines, LineKind, TomlLine};

fn scan(text: &str) -> Vec<TomlLine> {
    scan_toml_lines(text).lines
}

fn kinds(lines: &[TomlLine]) -> Vec<&'static str> {
    lines
        .iter()
        .map(|line| match line.kind {
            LineKind::Section => "section",
            LineKind::Entry => "entry",
            LineKind::Text => "text",
        })
        .collect()
}

#[test]
fn quoted_values_lose_quotes_and_keep_inner_hash() {
    assert_eq!(parse_toml_value("\"http://127.0.0.1:18888/v1\""), "http://127.0.0.1:18888/v1");
    assert_eq!(parse_toml_value("'single'"), "single");
    assert_eq!(parse_toml_value("\"#0169cc\""), "#0169cc");
    assert_eq!(parse_toml_value("true"), "true");
    assert_eq!(parse_toml_value("  42  "), "42");
    // 转义只在双引号里算转义，\" 里的引号不该提前收尾
    assert_eq!(parse_toml_value("\"a\\\"b\""), "a\\\"b");
    assert_eq!(parse_toml_value("\"a\\\"b\" # 尾注释"), "a\\\"b");
}

#[test]
fn trailing_comments_are_stripped() {
    assert_eq!(parse_toml_value("http://127.0.0.1:18888 # 本地代理"), "http://127.0.0.1:18888");
    assert_eq!(parse_toml_value("\"http://127.0.0.1:18888\" # 本地代理"), "http://127.0.0.1:18888");
    assert_eq!(parse_toml_value("# 整行注释"), "");
}

#[test]
fn unclosed_quote_degrades_to_bare_value() {
    assert_eq!(parse_toml_value("\"unclosed"), "unclosed");
}

#[test]
fn sections_top_level_keys_and_section_keys_are_distinct() {
    let lines = scan(&[
        "model_provider = \"custom\"",
        "model = \"gpt-5.6-sol\"",
        "",
        "# 注释",
        "[model_providers.custom]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
    ]
    .join("\n"));

    assert_eq!(kinds(&lines), vec!["entry", "entry", "text", "text", "section", "entry"]);
    assert_eq!(lines[0].section, "");
    assert_eq!(lines[0].key, "model_provider");
    assert_eq!(lines[0].value, "custom");
    assert_eq!(lines[4].section, "model_providers.custom");
    assert_eq!(lines[5].section, "model_providers.custom");
    assert_eq!(lines[5].value, "http://127.0.0.1:18888/v1");
}

#[test]
fn section_name_with_trailing_comment_still_counts() {
    let lines = scan(&[
        "[model_providers.custom] # 我自己的备用供应商",
        "base_url = \"https://api.example.com/v1\"",
    ]
    .join("\n"));
    assert_eq!(lines[0].kind, LineKind::Section);
    assert_eq!(lines[0].section, "model_providers.custom");
    // 节名行之后的键值要归到它名下，不能因为认不出节名就当成顶层键
    assert_eq!(lines[1].kind, LineKind::Entry);
    assert_eq!(lines[1].section, "model_providers.custom");
}

#[test]
fn section_paths_are_normalized() {
    assert_eq!(scan("[model_providers.\"ccproxy\"]\n")[0].section, "model_providers.ccproxy");
    assert_eq!(scan("[model_providers.'ccproxy']\n")[0].section, "model_providers.ccproxy");
    assert_eq!(scan("[a.b]\n")[0].section, "a.b");
    // 引号里的 ] 属于名字本身，不能当成节名的结尾
    assert_eq!(scan("[model_providers.\"a]b\"]\nx = 1\n")[0].section, "model_providers.a]b");
}

#[test]
fn unterminated_multiline_string_is_flagged() {
    let broken = scan_toml_lines("model_provider = \"custom\"\ndev = \"\"\"\nmodel_provider = \"hijacked\"\n");
    assert!(broken.unterminated);
    // 未闭合时后面那些行只能当字符串内容，不能当键值
    assert!(!broken.lines.iter().any(|line| line.value == "hijacked"));

    assert!(!scan_toml_lines("dev = \"\"\"\n内容\n\"\"\"\nmodel_provider = \"custom\"\n").unterminated);
    assert!(!scan_toml_lines("dev = \"\"\"一行闭合\"\"\"\n").unterminated);
}

#[test]
fn double_quoted_multiline_block_hides_keys_and_sections() {
    let lines = scan(&[
        "model_provider = \"custom\"",
        "developer_instructions = \"\"\"",
        "model_provider = \"hijacked\"",
        "[model_providers.hijack]",
        "\"\"\"",
        "[model_providers.custom]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
    ]
    .join("\n"));

    assert_eq!(kinds(&lines[2..4]), vec!["text", "text"]);
    let top_level = lines
        .iter()
        .find(|line| line.section.is_empty() && line.key == "model_provider")
        .expect("应有顶层 model_provider");
    assert_eq!(top_level.value, "custom");
    assert!(!lines.iter().any(|line| line.value == "hijacked"));
    assert!(!lines.iter().any(|line| line.section == "model_providers.hijack"));
}

#[test]
fn single_quoted_multiline_block_is_also_isolated() {
    let lines = scan(&[
        "instructions = '''",
        "model_provider = \"hijacked\"",
        "'''",
        "[model_providers.custom]",
        "base_url = \"http://127.0.0.1:18888/v1\"",
    ]
    .join("\n"));
    assert_eq!(kinds(&lines[1..2]), vec!["text"]);
    assert!(!lines.iter().any(|line| line.value == "hijacked"));
}

#[test]
fn same_line_multiline_delimiter_does_not_enter_multiline_state() {
    let lines = scan(&["note = \"\"\"一行写完\"\"\"", "model_provider = \"custom\""].join("\n"));
    assert_eq!(kinds(&lines), vec!["entry", "entry"]);
    assert_eq!(lines[1].key, "model_provider");
}

#[test]
fn indentation_and_crlf_are_handled() {
    let lines = scan("  [model_providers.custom]\r\n\tbase_url = \"x\"  \r\n");
    assert_eq!(lines[0].kind, LineKind::Section);
    assert_eq!(lines[0].section, "model_providers.custom");
    assert_eq!(lines[1].kind, LineKind::Entry);
    assert_eq!(lines[1].value, "x");
    assert_eq!(lines[1].eol, "\r\n");
}

#[test]
fn unrecognized_lines_are_text() {
    let lines = scan(&["not a toml line", "= 5", "[]", "[[array]]", "[a] b"].join("\n"));
    assert_eq!(kinds(&lines), vec!["text", "text", "text", "text", "text"]);
    let entries = scan("key = value")
        .into_iter()
        .filter(|line| line.kind == LineKind::Entry)
        .count();
    assert_eq!(entries, 1);
}

