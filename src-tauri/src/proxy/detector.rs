// 协议识别器：只按请求路径判定请求属于哪种 agent 协议。
// 原来的打分制（路径 + 请求头 + 请求体特征）已去掉：真实运行日志里路径单独就决定了
// 全部判定，头/体特征一次都没改变过结论，却带来「打分并列 → 400」这种解释不清的失败。
// 未命中的路径直接按未知协议报错，文案见 unknown_path_message。
// 请求体解析（转发链取 clientModel 用）也放在这里。

use serde_json::Value;

// 认识的路径。取前缀是为了让 /v1/messages/count_tokens 这类子路径一起进来
const ANTHROPIC_PATH_PREFIX: &str = "/v1/messages";
const OPENAI_PATH_PREFIXES: [&str; 5] = [
    "/v1/chat/completions",
    "/v1/responses",
    "/v1/completions",
    "/v1/embeddings",
    "/v1/models",
];

// 前缀按整段对齐：/v1/messagesx 不是 Anthropic 的路径
fn is_path_prefix(pathname: &str, prefix: &str) -> bool {
    match pathname.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

// 识别入口：None 表示这条路径不属于任何已知协议
pub fn detect_protocol(path: &str) -> Option<&'static str> {
    let pathname = path.split('?').next().unwrap_or("");
    if is_path_prefix(pathname, ANTHROPIC_PATH_PREFIX) {
        return Some("anthropic");
    }
    if OPENAI_PATH_PREFIXES
        .iter()
        .copied()
        .any(|prefix| is_path_prefix(pathname, prefix))
    {
        return Some("openai");
    }
    None
}

// 400 文案用上面两份清单拼出来，免得提示与判定的路径各写一套、改一处忘一处
pub fn unknown_path_message(path: &str) -> String {
    format!(
        "路径 {path} 不属于任何已知协议：Anthropic 用 {ANTHROPIC_PATH_PREFIX}*，OpenAI 用 {}",
        OPENAI_PATH_PREFIXES.join("、")
    )
}

// 解析请求体为 JSON 对象；失败或非对象返回 None，绝不抛异常
pub fn parse_body(body: &[u8]) -> Option<Value> {
    if body.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_slice(body).ok()?;
    if parsed.is_object() {
        Some(parsed)
    } else {
        None
    }
}
