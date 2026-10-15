// 转换入口：把「客户端格式 → 上游格式」的请求、响应、流式三件事收敛在这里，
// 转发管线只按格式对调用，不直接碰下面那些模块。
//
// 上游格式来自供应商声明的 apiFormat（auto 已在 router 解析成客户端格式），
// 两边相同表示不做转换，调用方直接透传。
//
// 还没有实现的方向（客户端 openai_chat → 上游 anthropic / openai_responses）返回 Err，
// 由调用方按「这个组合不支持」报给用户：静默透传会把 Chat 请求发给只认别的格式的上游，
// 那时上游报的错比这里难懂得多。

use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use futures::stream::Stream;
use serde_json::Value;

use crate::convert::providers::{
    self, codex_chat_history::CodexChatHistoryStore, streaming, streaming_codex_anthropic, streaming_codex_chat,
    streaming_responses, transform, transform_codex_anthropic, transform_codex_chat, transform_responses,
};

// Responses → Anthropic 时 Anthropic 的 max_tokens 是必填项，缺省给这个值（与上游 cc-switch 同值）
const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 8192;

// Codex 的历史 store：多轮里客户端可能只回传 tool_call 的 id 而不带参数，靠它补齐。
// 它按响应 id 自键（不靠外部会话参数），进程内一份就够，所以放在这里而不用穿到 AppState。
fn history_store() -> Arc<CodexChatHistoryStore> {
    static STORE: OnceLock<Arc<CodexChatHistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| Arc::new(CodexChatHistoryStore::default())).clone()
}

pub fn needs_conversion(client: &str, upstream: &str) -> bool {
    client != upstream
}

// 客户端要流式、上游却回了整块 JSON 时，把响应展开成客户端协议的完整 SSE 生命周期。
// 这是给「上游忽略 stream 参数」收尾的：直接把 JSON 丢给等 SSE 的客户端，它只会当成空响应
pub fn json_to_sse(client: &str, value: &Value) -> Result<Vec<Bytes>, String> {
    crate::convert::stream::json_response_to_sse(client, value)
}

/// 2xx 里带 error 的情况（网关把失败包在 200 里）：这种不能展开成「正常但空」的 SSE，
/// 得原样交给客户端，让它看见失败
pub fn is_error_envelope(value: &Value) -> bool {
    match value.get("error") {
        None | Some(Value::Null) => false,
        Some(Value::Object(map)) => !map.is_empty(),
        Some(_) => true,
    }
}

// 上游报错时把错误体换成客户端那一套形状。转格式之后错误体也跟着换了语言：
// Claude Code 只认 {"type":"error","error":{...}}，Codex 只认 {"error":{...}}，
// 把上游那份原样丢过去，用户看到的是一句解析不了的东西。认不出文案时返回 None，
// 调用方原样透传——宁可难看也别把失败原因编没了
pub fn error_body_for_client(client: &str, status: u16, body: &[u8]) -> Option<Vec<u8>> {
    let raw = String::from_utf8_lossy(body);
    let parsed = serde_json::from_str::<Value>(&raw).ok();
    let message = extract_error_message(parsed.as_ref(), &raw)?;
    let error_type = error_type_for_client(client, status);
    let payload = if client == "anthropic" {
        serde_json::json!({ "type": "error", "error": { "type": error_type, "message": message } })
    } else {
        serde_json::json!({ "error": { "message": message, "type": error_type } })
    };
    serde_json::to_vec(&payload).ok()
}

// 从各家错误体里捞出给人看的那句话：OpenAI 与 Anthropic 都是 error.message，
// 顶层 message 与裸字符串也认（网关自己造的错）；都没有就退回原始文本
fn extract_error_message(parsed: Option<&Value>, raw: &str) -> Option<String> {
    if let Some(value) = parsed {
        for pointer in ["/error/message", "/error/error/message", "/message", "/error"] {
            if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    let trimmed = raw.trim();
    // HTML 错误页（反代直接吐的那种）当文案没有价值，不如原样透传让用户自己看
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return None;
    }
    Some(trimmed.chars().take(600).collect())
}

// 错误类型的取名也是各协议一套：Anthropic 有 permission_error / not_found_error，
// OpenAI 一族习惯用 invalid_api_key / rate_limit_exceeded 这类词
fn error_type_for_client(client: &str, status: u16) -> &'static str {
    let anthropic = client == "anthropic";
    match status {
        401 => {
            if anthropic {
                "authentication_error"
            } else {
                "invalid_api_key"
            }
        }
        403 => {
            if anthropic {
                "permission_error"
            } else {
                "permission_denied"
            }
        }
        404 => {
            if anthropic {
                "not_found_error"
            } else {
                "invalid_request_error"
            }
        }
        429 => {
            if anthropic {
                "rate_limit_error"
            } else {
                "rate_limit_exceeded"
            }
        }
        400..=499 => "invalid_request_error",
        _ => "api_error",
    }
}

/// 上游字节流的错误：透传时来自 reqwest、转换时来自状态机，汇成同一个类型。
/// 转换器那边要求错误类型实现 std::error::Error，而管线里传的是 String，
/// 所以这里做个薄包装；真正的错误文本原样带着，不加工
#[derive(Debug)]
pub struct StreamError(pub String);

impl std::fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StreamError {}

fn pair(client: &str, upstream: &str) -> String {
    format!("客户端 {client} → 上游 {upstream}")
}

fn unsupported(client: &str, upstream: &str) -> String {
    format!(
        "{} 的协议转换尚未实现：这个供应商的上游格式与客户端协议不同，换一个上游格式或换一个供应商",
        pair(client, upstream)
    )
}

// 请求侧：把客户端请求体转成上游格式
pub fn convert_request(client: &str, upstream: &str, body: &Value) -> Result<Value, String> {
    let result = match (client, upstream) {
        ("anthropic", "openai_chat") => transform::anthropic_to_openai_with_reasoning_content(body.clone(), false),
        ("anthropic", "openai_responses") => {
            transform_responses::anthropic_to_responses(body.clone(), None, false, false)
        }
        ("openai_responses", "openai_chat") => {
            transform_codex_chat::responses_to_chat_completions_with_reasoning(body.clone(), None)
        }
        ("openai_responses", "anthropic") => {
            transform_codex_anthropic::responses_request_to_anthropic(body.clone(), DEFAULT_ANTHROPIC_MAX_TOKENS)
        }
        _ => return Err(unsupported(client, upstream)),
    };
    result.map_err(|error| format!("{} 的请求转换失败：{error}", pair(client, upstream)))
}

// Codex 的请求转 Chat 之前先补历史：客户端只回传 tool_call id 时，参数得从上一轮找回来
pub async fn enrich_request(client: &str, upstream: &str, body: &mut Value) {
    if client == "openai_responses" && upstream == "openai_chat" {
        history_store().enrich_request(body).await;
    }
}

// 响应侧：把上游响应体转回客户端格式。original_request 是**客户端原始请求体**——
// Responses 那条线要拿它还原被扁平化的工具名（namespace / custom / tool_search）
pub async fn convert_response(
    client: &str,
    upstream: &str,
    original_request: &Value,
    body: Value,
) -> Result<Value, String> {
    let tool_context = || transform_codex_chat::build_codex_tool_context_from_request(original_request);
    let result = match (client, upstream) {
        ("anthropic", "openai_chat") => transform::openai_to_anthropic(body),
        ("anthropic", "openai_responses") => transform_responses::responses_to_anthropic(body),
        ("openai_responses", "openai_chat") => {
            transform_codex_chat::chat_completion_to_response_with_context(body, &tool_context())
        }
        ("openai_responses", "anthropic") => {
            transform_codex_anthropic::anthropic_response_to_responses_with_context(body, &tool_context())
        }
        _ => return Err(unsupported(client, upstream)),
    };
    let converted = result.map_err(|error| format!("{} 的响应转换失败：{error}", pair(client, upstream)))?;
    // 记一笔供下一轮补历史；只在 Codex 这条线上有意义
    if client == "openai_responses" {
        history_store().record_response(&converted).await;
    }
    Ok(converted)
}

// 流式：上游 SSE 边收边转成客户端格式的 SSE
pub fn convert_stream(
    client: &str,
    upstream: &str,
    original_request: &Value,
    stream: impl Stream<Item = Result<Bytes, StreamError>> + Send + 'static,
) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>, String>
{
    let converted: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> = match (client, upstream) {
        ("anthropic", "openai_chat") => Box::pin(streaming::create_anthropic_sse_stream(stream)),
        ("anthropic", "openai_responses") => {
            Box::pin(streaming_responses::create_anthropic_sse_stream_from_responses(stream))
        }
        ("openai_responses", "openai_chat") => {
            let context = transform_codex_chat::build_codex_tool_context_from_request(original_request);
            let converted = streaming_codex_chat::create_responses_sse_stream_from_chat_with_context(stream, context);
            Box::pin(providers::codex_chat_history::record_responses_sse_stream(
                converted,
                history_store(),
            ))
        }
        ("openai_responses", "anthropic") => {
            let context = transform_codex_chat::build_codex_tool_context_from_request(original_request);
            let converted =
                streaming_codex_anthropic::create_responses_sse_stream_from_anthropic_with_context(stream, context);
            Box::pin(providers::codex_chat_history::record_responses_sse_stream(
                converted,
                history_store(),
            ))
        }
        _ => return Err(unsupported(client, upstream)),
    };
    Ok(converted)
}
