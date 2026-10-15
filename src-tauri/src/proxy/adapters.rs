// 协议适配器：收敛「认证怎么注入」「model 字段改在哪」这类协议差异，转发逻辑里不写死任何协议细节。
// 响应侧对称地做 model 回写：请求把 clientModel 换成 realModel 发往上游，响应把真实模型名
// 换回 clientModel 还给客户端，保证客户端看到的请求/响应模型名一致。

use std::sync::Mutex;

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::{json, Value};

// 固定剥离清单：无论什么协议，入站请求携带的凭据一律摘掉，保证客户端的假 key 绝不外泄
pub const STRIP_INBOUND_HEADERS: [&str; 5] =
    ["authorization", "x-api-key", "api-key", "x-goog-api-key", "proxy-authorization"];

// 就地剥离入站凭据
pub fn strip_inbound_credentials(headers: &mut HeaderMap) {
    for name in STRIP_INBOUND_HEADERS {
        headers.remove(name);
    }
}

// 上游一次上报里认得的字段，input 是它原样给的那一项：anthropic 的 input_tokens 不含
// 缓存读写，openai 的 input_tokens / prompt_tokens 已经是含缓存的总额
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Reported {
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    // anthropic 的「写进缓存」那一项：只用来把全部输入算出来，不单独对外（openai 没有这项）
    cache_write: Option<i64>,
}

impl Reported {
    fn is_empty(&self) -> bool {
        self.input.is_none() && self.output.is_none() && self.cache_read.is_none() && self.cache_write.is_none()
    }
}

// 对外口径（日志页与用量页都看这一份）：input 统一成「这次请求的全部输入」，
// cache_read 是它里面命中缓存的那部分——anthropic 那边要自己把缓存读写加进 input_tokens，
// openai 那边本来就含，不能加第二遍（加了就是重复计数）
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
}

impl Usage {
    fn from_reported(reported: Reported, input_includes_cache: bool) -> Self {
        Self {
            input: if input_includes_cache {
                reported.input
            } else {
                sum_present([reported.input, reported.cache_read, reported.cache_write])
            },
            output: reported.output,
            cache_read: reported.cache_read,
        }
    }

    // 字段名在这里定一次：日志与用量 JSONL 落盘、前端读的都是这几个 key
    pub fn to_json(self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cacheRead": self.cache_read
        })
    }
}

// 全都没上报就是 None，与「上报了 0」分开
fn sum_present(values: [Option<i64>; 3]) -> Option<i64> {
    let mut total: Option<i64> = None;
    for value in values.into_iter().flatten() {
        total = Some(total.unwrap_or(0) + value);
    }
    total
}

// 用量计数字段可能缺、可能是 null、可能上游给了字符串：认不出的都当「没上报」
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
fn merge_usage(target: &mut Reported, next: Option<Reported>) {
    let Some(next) = next else { return };
    if next.input.is_some() {
        target.input = next.input;
    }
    if next.output.is_some() {
        target.output = next.output;
    }
    if next.cache_read.is_some() {
        target.cache_read = next.cache_read;
    }
    if next.cache_write.is_some() {
        target.cache_write = next.cache_write;
    }
}

fn as_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

// 只在原处确实有 model 字符串时才替换：没有这个字段的事件（多数字增量事件都没有）
// 不该被凭空塞进一个 model
fn replace_model(scope: Option<&mut serde_json::Map<String, Value>>, client_model: &str) {
    let Some(scope) = scope else { return };
    if scope.get("model").and_then(Value::as_str).is_some() {
        scope.insert("model".to_string(), Value::String(client_model.to_string()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
}

impl Protocol {
    pub fn id(&self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::OpenAiChat => "openai_chat",
            Protocol::OpenAiResponses => "openai_responses",
        }
    }

    // 上游端点：客户端路径与上游格式同族时原样带上（/v1/messages/count_tokens 这类子路径
    // 要保住），不同族时换成目标格式的规范端点，查询串跟着走
    pub fn upstream_path(&self, client_path: &str) -> String {
        let pathname = client_path.split('?').next().unwrap_or("");
        let query = &client_path[pathname.len()..];
        let same_family = match self {
            Protocol::Anthropic => pathname.starts_with("/v1/messages"),
            Protocol::OpenAiChat => {
                pathname.starts_with("/v1/chat/completions")
                    || pathname.starts_with("/v1/completions")
                    || pathname.starts_with("/v1/embeddings")
            }
            Protocol::OpenAiResponses => pathname.starts_with("/v1/responses"),
        };
        if same_family {
            return client_path.to_string();
        }
        let endpoint = match self {
            Protocol::Anthropic => "/v1/messages",
            Protocol::OpenAiChat => "/v1/chat/completions",
            Protocol::OpenAiResponses => "/v1/responses",
        };
        format!("{endpoint}{query}")
    }

    // 两种协议的 input 口径不同：anthropic 的 input_tokens 只算没走缓存的那部分
    // （缓存读写在 cache_read_input_tokens / cache_creation_input_tokens 里另给），
    // openai 的 input_tokens / prompt_tokens 本身就是含缓存的总额
    fn input_includes_cache(&self) -> bool {
        match self {
            Protocol::Anthropic => false,
            Protocol::OpenAiChat | Protocol::OpenAiResponses => true,
        }
    }

    // 认证按**上游**格式注入（不是按客户端协议）：转了格式之后，凭据形态得跟着上游走。
    // Anthropic 用 x-api-key，且缺 anthropic-version 时补一个默认值——客户端是 Codex 时
    // 它不会带这个头，Anthropic 上游会拒
    pub fn apply_auth(&self, headers: &mut HeaderMap, token: &str) {
        match self {
            Protocol::Anthropic => {
                if let Ok(value) = axum::http::HeaderValue::from_str(token) {
                    headers.insert("x-api-key", value);
                }
                if !headers.contains_key("anthropic-version") {
                    headers.insert("anthropic-version", axum::http::HeaderValue::from_static("2023-06-01"));
                }
            }
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
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
    // openai 的 chat completions 是每个 chunk 顶层带 model，responses 则把整个响应对象
    // 装在 response 里、model 在 response.model（实测网关的 created / in_progress /
    // 终止事件都是这个形状，事件顶层没有 model）
    fn rewrite_stream_data_model(&self, payload: &mut Value, client_model: &str) {
        match self {
            Protocol::Anthropic => {
                let is_start = payload.get("type").and_then(Value::as_str) == Some("message_start");
                if is_start {
                    replace_model(payload.get_mut("message").and_then(Value::as_object_mut), client_model);
                }
            }
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                replace_model(payload.as_object_mut(), client_model);
                replace_model(payload.get_mut("response").and_then(Value::as_object_mut), client_model);
            }
        }
    }

    // 用量：anthropic 非流式在顶层 usage，流式里输入在 message_start.message.usage、
    // 输出的最终值在 message_delta.usage；openai 有两种线格式——chat completions 用
    // usage.prompt_tokens / completion_tokens（流式只出现在最后一个 chunk），
    // responses（Codex 的 wire_api，路径 /v1/responses）非流式在顶层 usage、
    // 流式挂在 response.completed 的 response.usage 里，字段是 input_tokens / output_tokens
    fn extract_usage(&self, payload: &Value) -> Option<Reported> {
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
                Some(Reported {
                    input: count_tokens(source.get("input_tokens")),
                    output: count_tokens(source.get("output_tokens")),
                    cache_read: count_tokens(source.get("cache_read_input_tokens")),
                    cache_write: count_tokens(source.get("cache_creation_input_tokens")),
                })
            }
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                // responses 的流式事件把用量挂在 response 里（response.completed），
                // 非流式与 chat completions 一样在顶层
                let nested = payload
                    .get("response")
                    .and_then(as_object)
                    .and_then(|response| response.get("usage"))
                    .and_then(as_object);
                let usage = source.get("usage").and_then(as_object).or(nested)?;
                Some(Reported {
                    input: count_tokens(usage.get("prompt_tokens")).or_else(|| count_tokens(usage.get("input_tokens"))),
                    output: count_tokens(usage.get("completion_tokens")).or_else(|| count_tokens(usage.get("output_tokens"))),
                    cache_read: cached_tokens(usage),
                    // openai 没有「写缓存」这个概念
                    cache_write: None,
                })
            }
        }
    }
}

// 缓存命中：responses 在 input_tokens_details.cached_tokens、chat completions 在
// prompt_tokens_details.cached_tokens，这是 OpenAI 家的标准位置；
// prompt_cache_hit_tokens 是 deepseek 一派网关的写法，顺手也认
fn cached_tokens(usage: &serde_json::Map<String, Value>) -> Option<i64> {
    let details = |name: &str| {
        usage
            .get(name)
            .and_then(as_object)
            .and_then(|detail| count_tokens(detail.get("cached_tokens")))
    };
    details("input_tokens_details")
        .or_else(|| details("prompt_tokens_details"))
        .or_else(|| count_tokens(usage.get("prompt_cache_hit_tokens")))
}

pub fn get_adapter(protocol_id: &str) -> Result<Protocol, String> {
    match protocol_id {
        "anthropic" => Ok(Protocol::Anthropic),
        "openai_chat" => Ok(Protocol::OpenAiChat),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        other => Err(format!("未知协议: {other}")),
    }
}

// 生成响应回写器：非流式改 JSON 顶层 model，流式逐 SSE 事件改 data 行。
// 非 JSON（如上游错误页）或没有 model 字段的一律原样透传，绝不因为回写失败弄坏响应。
// 回写器同时兼管用量观察：它是唯一按协议语义解析响应体的地方
pub struct ResponseRewriter {
    protocol: Protocol,
    client_model: String,
    usage: Mutex<Reported>,
}

impl ResponseRewriter {
    pub fn new(protocol: Protocol, client_model: &str) -> Self {
        Self {
            protocol,
            client_model: client_model.to_string(),
            usage: Mutex::new(Reported::default()),
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
        let reported = *self.usage.lock().ok()?;
        if reported.is_empty() {
            return None;
        }
        Some(Usage::from_reported(reported, self.protocol.input_includes_cache()))
    }
}

