// 协议适配器：收敛「认证怎么注入」「model 字段改在哪」这类协议差异，转发逻辑里不写死任何协议细节。
// 响应侧对称地做 model 回写：请求把 clientModel 换成 realModel 发往上游，响应把真实模型名
// 换回 clientModel 还给客户端，保证客户端看到的请求/响应模型名一致。

use std::sync::Mutex;

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;

// 固定剥离清单：无论什么协议，入站请求携带的凭据一律摘掉，保证客户端的假 key 绝不外泄
pub const STRIP_INBOUND_HEADERS: [&str; 5] =
    ["authorization", "x-api-key", "api-key", "x-goog-api-key", "proxy-authorization"];

// 就地剥离入站凭据
pub fn strip_inbound_credentials(headers: &mut HeaderMap) {
    for name in STRIP_INBOUND_HEADERS {
        headers.remove(name);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: Option<i64>,
    pub output: Option<i64>,
}

// 用量计数字段可能缺、可能是 null、可能上游给了字符串：认不出的都当「没上报」。
// 只有输入/输出两项——用量页按这两项聚合与计价，缓存类 tokens 暂不纳入
fn count_tokens(value: Option<&Value>) -> Option<i64> {
    let number = match value? {
        Value::Null => 0.0,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                0.0
            } else {
                trimmed.parse::<f64>().ok()?
            }
        }
        _ => return None,
    };
    if number.is_finite() && number >= 0.0 {
        Some(number as i64)
    } else {
        None
    }
}

// 把一次上报里认得的字段并进累计器。后到的覆盖先到的，不累加：
// anthropic 的 output 在 message_start 里是起步值、在 message_delta 里是最终累计值，
// 累加会把 output 算成好几倍
fn merge_usage(target: &mut Usage, next: Option<Usage>) {
    let Some(next) = next else { return };
    if next.input.is_some() {
        target.input = next.input;
    }
    if next.output.is_some() {
        target.output = next.output;
    }
}

fn as_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Anthropic,
    Openai,
}

impl Protocol {
    pub fn id(&self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::Openai => "openai",
        }
    }

    // Anthropic 协议规范认证方式：x-api-key 头；OpenAI 协议规范认证方式：Authorization Bearer
    pub fn apply_auth(&self, headers: &mut HeaderMap, token: &str) {
        match self {
            Protocol::Anthropic => {
                if let Ok(value) = axum::http::HeaderValue::from_str(token) {
                    headers.insert("x-api-key", value);
                }
            }
            Protocol::Openai => {
                if let Ok(value) = axum::http::HeaderValue::from_str(&format!("Bearer {token}")) {
                    headers.insert("authorization", value);
                }
            }
        }
    }

    pub fn rewrite_model(&self, payload: &mut Value, real_model: &str) {
        if let Some(object) = payload.as_object_mut() {
            object.insert("model".to_string(), Value::String(real_model.to_string()));
        }
    }

    // 流式响应里 model 只出现在 message_start 事件的 message.model 上（anthropic）；
    // openai 的每个 chunk 顶层都带 model
    fn rewrite_stream_data_model(&self, payload: &mut Value, client_model: &str) {
        match self {
            Protocol::Anthropic => {
                let is_start = payload.get("type").and_then(Value::as_str) == Some("message_start");
                if is_start {
                    if let Some(message) = payload.get_mut("message").and_then(Value::as_object_mut) {
                        if message.get("model").and_then(Value::as_str).is_some() {
                            message.insert("model".to_string(), Value::String(client_model.to_string()));
                        }
                    }
                }
            }
            Protocol::Openai => {
                if let Some(object) = payload.as_object_mut() {
                    if object.get("model").and_then(Value::as_str).is_some() {
                        object.insert("model".to_string(), Value::String(client_model.to_string()));
                    }
                }
            }
        }
    }

    // 用量：非流式在顶层 usage；流式里输入在 message_start.message.usage，
    // 输出的最终值在 message_delta.usage
    fn extract_usage(&self, payload: &Value) -> Option<Usage> {
        let source = as_object(payload)?;
        match self {
            Protocol::Anthropic => {
                let message_usage = if payload.get("type").and_then(Value::as_str) == Some("message_start") {
                    payload
                        .get("message")
                        .and_then(as_object)
                        .and_then(|message| message.get("usage"))
                        .and_then(as_object)
                } else {
                    None
                };
                let source = message_usage.or_else(|| source.get("usage").and_then(as_object))?;
                Some(Usage {
                    input: count_tokens(source.get("input_tokens")),
                    output: count_tokens(source.get("output_tokens")),
                })
            }
            Protocol::Openai => {
                let usage = source.get("usage").and_then(as_object)?;
                Some(Usage {
                    input: count_tokens(usage.get("prompt_tokens")),
                    output: count_tokens(usage.get("completion_tokens")),
                })
            }
        }
    }
}

pub fn get_adapter(protocol_id: &str) -> Result<Protocol, String> {
    match protocol_id {
        "anthropic" => Ok(Protocol::Anthropic),
        "openai" => Ok(Protocol::Openai),
        other => Err(format!("未知协议: {other}")),
    }
}

// 生成响应回写器：非流式改 JSON 顶层 model，流式逐 SSE 事件改 data 行。
// 非 JSON（如上游错误页）或没有 model 字段的一律原样透传，绝不因为回写失败弄坏响应。
// 回写器同时兼管用量观察：它是唯一按协议语义解析响应体的地方
pub struct ResponseRewriter {
    protocol: Protocol,
    client_model: String,
    usage: Mutex<Usage>,
}

impl ResponseRewriter {
    pub fn new(protocol: Protocol, client_model: &str) -> Self {
        Self {
            protocol,
            client_model: client_model.to_string(),
            usage: Mutex::new(Usage::default()),
        }
    }

    // 非流式：整体一个 JSON，顶层 model 回写
    pub fn rewrite_body(&self, buffer: &Bytes) -> Bytes {
        let Ok(payload) = serde_json::from_slice::<Value>(buffer) else {
            return buffer.clone();
        };
        if let Ok(mut usage) = self.usage.lock() {
            merge_usage(&mut usage, self.protocol.extract_usage(&payload));
        }
        if payload.get("model").and_then(Value::as_str) == Some(self.client_model.as_str()) {
            return buffer.clone();
        }
        if payload.get("model").and_then(Value::as_str).is_none() {
            return buffer.clone();
        }
        let mut payload = payload;
        if let Some(object) = payload.as_object_mut() {
            object.insert("model".to_string(), Value::String(self.client_model.clone()));
        }
        match serde_json::to_vec(&payload) {
            Ok(encoded) => Bytes::from(encoded),
            Err(_) => buffer.clone(),
        }
    }

    // 流式：一个完整 SSE 事件（含结尾空行），只改写 data: 行里的 JSON
    pub fn rewrite_sse_event(&self, event: &[u8]) -> Bytes {
        let text = String::from_utf8_lossy(event);
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let mut changed = false;
        for line in lines.iter_mut() {
            let Some(rest) = line.strip_prefix("data:") else {
                continue;
            };
            let Ok(mut payload) = serde_json::from_str::<Value>(rest) else {
                // 非 JSON 的 data 行（如 [DONE]）原样保留
                continue;
            };
            if let Ok(mut usage) = self.usage.lock() {
                merge_usage(&mut usage, self.protocol.extract_usage(&payload));
            }
            self.protocol.rewrite_stream_data_model(&mut payload, &self.client_model);
            if let Ok(encoded) = serde_json::to_string(&payload) {
                *line = format!("data: {encoded}");
                changed = true;
            }
        }
        if !changed {
            return Bytes::copy_from_slice(event);
        }
        Bytes::from(lines.join("\n").into_bytes())
    }

    // 一个字段都没收到时返回 None：日志与用量页要能区分「上游没上报」和「上报了 0」
    pub fn usage(&self) -> Option<Usage> {
        let usage = self.usage.lock().ok()?;
        if usage.input.is_none() && usage.output.is_none() {
            None
        } else {
            Some(*usage)
        }
    }
}

