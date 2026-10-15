// 单请求处理链：收体 → 模型列表接管 → 协议识别 → 查映射表 → 改写 → 转发 → 埋点。

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Response};
use bytes::{Bytes, BytesMut};
use futures_util::{stream, Stream, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};

use crate::proxy::adapters::{get_adapter, strip_inbound_credentials, ResponseRewriter};
use crate::proxy::detector::{detect_protocol, parse_body, unknown_path_message};
use crate::proxy::models::{build_models_response, match_models_request, resolve_client_protocol};
use crate::proxy::router::{resolve_routing, Route};
use crate::proxy::upstream::{
    build_upstream_url, passthrough_response_headers, sanitize_outbound_headers, send_upstream, NON_STREAM_TIMEOUT_MS,
    STREAM_IDLE_TIMEOUT_MS,
};
use crate::convert::bridge as convert_bridge;
use crate::state::AppState;

pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

// 每条请求一个进程内唯一的 id：日志页要拿它当列表 key，而同一毫秒里进来两条请求
// （并发客户端很常见）时间戳会撞，拿时间戳当 key 会串行
static REQUEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_request_id(started_at: i64) -> String {
    let seq = REQUEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("{started_at}-{seq}")
}

#[derive(Default, Clone)]
struct LogFields {
    protocol: Option<String>,
    client_model: Option<String>,
    match_kind: Option<String>,
    matched_rule: Option<String>,
    mapping_id: Option<String>,
    provider_id: Option<String>,
    provider_name: Option<String>,
    real_model: Option<String>,
    status_code: u16,
    outcome: String,
    error_code: Option<String>,
    error: Option<String>,
    cancelled: bool,
    tokens: Option<Value>,
}

// 统一日志 entry：五个分支（模型列表 / 收体失败 / 识别失败 / 查表失败 / 正常转发）字段必须一致，
// 否则日志页每个字段都要按分支判空
fn make_log_entry(id: &str, started_at: i64, method: &str, path: &str, fields: LogFields) -> Value {
    json!({
        "id": id,
        "timestamp": started_at,
        "durationMs": crate::now_ms() - started_at,
        "method": method,
        "path": path,
        "protocol": fields.protocol,
        "clientModel": fields.client_model,
        "matchKind": fields.match_kind,
        "matchedRule": fields.matched_rule,
        "mappingId": fields.mapping_id,
        "providerId": fields.provider_id,
        "providerName": fields.provider_name,
        "realModel": fields.real_model,
        "statusCode": fields.status_code,
        "outcome": if fields.outcome.is_empty() { "unknown".to_string() } else { fields.outcome },
        "errorCode": fields.error_code,
        "error": fields.error,
        "cancelled": fields.cancelled,
        "tokens": fields.tokens
    })
}

// 请求日志由 request_log 落盘并推给界面；读内存里的最新配置：开关一改当场生效，代理不用重启
fn emit_log(state: &AppState, entry: &Value) {
    let (logs, usage) = {
        let Ok(config) = state.config.read() else { return };
        (config.settings.logs_enabled, config.settings.usage_enabled)
    };
    if !logs && !usage {
        return;
    }
    state.request_log.record(entry, logs, usage);
    if logs {
        state.emit("log-appended", entry);
    }
}

fn json_response(status: u16, payload: &Value) -> Response<Body> {
    let body = serde_json::to_vec(payload).unwrap_or_default();
    let length = body.len();
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("content-length", length)
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

// 是否为 Anthropic 的 token 计数请求
fn is_count_tokens_path(path: &str) -> bool {
    path.split('?').next().unwrap_or("").starts_with("/v1/messages/count_tokens")
}

// 本地 token 估算：把请求体里所有字符串按 4 个字符 1 token 估。
// 只在「上游不是 Anthropic」时用：那时没有对方的 tokenizer 可用，而 Claude Code
// 拿这个数只做上下文预估，估个大概比把请求发给不认这个端点的上游强
fn estimate_input_tokens(payload: &Value) -> i64 {
    fn walk(value: &Value, chars: &mut usize) {
        match value {
            Value::String(text) => *chars += text.chars().count(),
            Value::Array(items) => items.iter().for_each(|item| walk(item, chars)),
            Value::Object(map) => map.values().for_each(|item| walk(item, chars)),
            _ => {}
        }
    }
    let mut chars = 0usize;
    walk(payload, &mut chars);
    (chars / 4).max(1) as i64
}

struct BodyError {
    status: u16,
    code: &'static str,
    message: String,
}

// 读完整请求体；超过上限报 413
async fn read_body(body: Body, limit: usize) -> Result<Bytes, BodyError> {
    let mut body = body;
    let mut collected = BytesMut::new();
    loop {
        match body.frame().await {
            None => break,
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    if collected.len() + data.len() > limit {
                        return Err(BodyError {
                            status: 413,
                            code: "body_too_large",
                            message: format!("请求体超过 {limit} 字节上限"),
                        });
                    }
                    collected.extend_from_slice(data);
                }
            }
            Some(Err(error)) => {
                return Err(BodyError {
                    status: 500,
                    code: "body_read_error",
                    message: error.to_string(),
                })
            }
        }
    }
    Ok(collected.freeze())
}

#[derive(Clone)]
struct ForwardOutcome {
    status_code: u16,
    error: Option<String>,
    cancelled: bool,
}

type FinishHook = Arc<Mutex<Option<Box<dyn FnOnce(ForwardOutcome) + Send>>>>;
type LogSink = Arc<dyn Fn(ForwardOutcome, Option<Value>) + Send + Sync>;

fn fire_hook(hook: &FinishHook, outcome: ForwardOutcome) {
    let Ok(mut slot) = hook.lock() else { return };
    if let Some(callback) = slot.take() {
        callback(outcome);
    }
}

// 流被中途丢掉（客户端断开）时补一次 cancelled 的埋点；正常收尾时闭包已被取走，这里是空操作
struct CancelGuard {
    hook: FinishHook,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        fire_hook(
            &self.hook,
            ForwardOutcome { status_code: 502, error: Some("客户端提前断开".to_string()), cancelled: true },
        );
    }
}

fn find_event_boundary(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|pair| pair == b"\n\n")
}

// 上游字节流：错误统一化成 String——透传时是 reqwest 的错误，转换时是转换器的 io 错误，
// 汇到一条错误通道里，下游只需要一种处理
type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>;

struct StreamState {
    upstream: UpstreamStream,
    pending: BytesMut,
    rewriter: Option<Arc<ResponseRewriter>>,
    guard: CancelGuard,
    finished: bool,
}

fn rewrite_event(rewriter: &Option<Arc<ResponseRewriter>>, event: Bytes) -> Bytes {
    match rewriter {
        Some(rewriter) => rewriter.rewrite_sse_event(&event),
        None => event,
    }
}

// 流式转发：按 SSE 事件边界（\n\n）凑齐一个事件改写一个，空闲超时按块重置。
// source 可能是上游原始 SSE（透传），也可能是转换器吐出的客户端格式 SSE——
// 两条路在这里汇合，后面的 model 回写与用量提取对它们一视同仁
fn make_stream(
    source: UpstreamStream,
    rewriter: Option<Arc<ResponseRewriter>>,
    hook: FinishHook,
    status: u16,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let state = StreamState {
        upstream: source,
        pending: BytesMut::new(),
        rewriter,
        guard: CancelGuard { hook },
        finished: false,
    };
    stream::unfold(state, move |mut state| async move {
        loop {
            if state.finished {
                return None;
            }
            if let Some(index) = find_event_boundary(&state.pending) {
                let event = state.pending.split_to(index + 2).freeze();
                let out = rewrite_event(&state.rewriter, event);
                return Some((Ok(out), state));
            }
            let next = tokio::time::timeout(
                Duration::from_millis(STREAM_IDLE_TIMEOUT_MS),
                state.upstream.next(),
            )
            .await;
            match next {
                Ok(Some(Ok(chunk))) => state.pending.extend_from_slice(&chunk),
                Ok(Some(Err(message))) => {
                    state.finished = true;
                    fire_hook(
                        &state.guard.hook,
                        ForwardOutcome { status_code: status, error: Some(message.clone()), cancelled: false },
                    );
                    return Some((Err(std::io::Error::other(message)), state));
                }
                Ok(None) => {
                    state.finished = true;
                    fire_hook(
                        &state.guard.hook,
                        ForwardOutcome { status_code: status, error: None, cancelled: false },
                    );
                    if state.pending.is_empty() {
                        return None;
                    }
                    let rest = std::mem::take(&mut state.pending).freeze();
                    let out = rewrite_event(&state.rewriter, rest);
                    return Some((Ok(out), state));
                }
                Err(_) => {
                    state.finished = true;
                    let message = format!("流式响应空闲超过 {STREAM_IDLE_TIMEOUT_MS}ms");
                    fire_hook(
                        &state.guard.hook,
                        ForwardOutcome { status_code: status, error: Some(message.clone()), cancelled: false },
                    );
                    return Some((Err(std::io::Error::other(message)), state));
                }
            }
        }
    })
}

// 单个请求的完整处理链
pub async fn handle(state: Arc<AppState>, request: Request<Body>) -> Response<Body> {
    let started_at = crate::now_ms();
    let id = next_request_id(started_at);
    let method = request.method().clone();
    let uri = request.uri().clone();
    let path = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    let headers = request.headers().clone();

    // 收体失败（超 64MB 上限、连接中断）也要留下日志 entry：否则这类请求在日志页凭空消失
    let body = match read_body(request.into_body(), state.max_body_bytes()).await {
        Ok(body) => body,
        Err(error) => {
            let entry = make_log_entry(
                &id,
                started_at,
                method.as_str(),
                &path,
                LogFields {
                    status_code: error.status,
                    outcome: "read_failed".to_string(),
                    error_code: Some(error.code.to_string()),
                    error: Some(error.message.clone()),
                    ..Default::default()
                },
            );
            emit_log(&state, &entry);
            return json_response(
                error.status,
                &json!({ "error": { "type": "ccproxy_body", "message": error.message } }),
            );
        }
    };

    // 模型列表接口由代理接管：不透传上游，按当前激活的映射直接生成响应
    if let Some(models_request) = match_models_request(method.as_str(), &path) {
        let protocol_id = resolve_client_protocol(&headers);
        let (status, payload) = {
            let Ok(config) = state.config.read() else {
                return json_response(500, &json!({ "error": { "type": "ccproxy_internal", "message": "配置不可读" } }));
            };
            build_models_response(&config, &protocol_id, models_request.model_id.as_deref())
        };
        let entry = make_log_entry(
            &id,
            started_at,
            method.as_str(),
            &path,
            LogFields {
                protocol: Some(protocol_id),
                client_model: models_request.model_id.clone(),
                status_code: status,
                outcome: "models".to_string(),
                ..Default::default()
            },
        );
        emit_log(&state, &entry);
        return json_response(status, &payload);
    }

    // 协议由请求路径决定；认不出的路径直接 400，文案里列出代理认识的路径让人照着改 baseURL
    let Some(protocol_id) = detect_protocol(&path) else {
        let message = unknown_path_message(&path);
        let entry = make_log_entry(
            &id,
            started_at,
            method.as_str(),
            &path,
            LogFields {
                status_code: 400,
                outcome: "detect_failed".to_string(),
                error_code: Some("unknown_path".to_string()),
                error: Some(message.clone()),
                ..Default::default()
            },
        );
        emit_log(&state, &entry);
        return json_response(400, &json!({ "error": { "type": "ccproxy_detect", "message": message } }));
    };

    let payload = parse_body(&body);
    let client_model = payload
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let is_streaming = payload
        .as_ref()
        .map(|value| value.get("stream").and_then(Value::as_bool) == Some(true))
        .unwrap_or(false)
        || headers
            .get("accept")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .contains("text/event-stream");

    let routing = {
        let Ok(config) = state.config.read() else {
            return json_response(500, &json!({ "error": { "type": "ccproxy_internal", "message": "配置不可读" } }));
        };
        resolve_routing(&config, protocol_id, &client_model)
    };
    let route: Route = match routing {
        Ok(route) => route,
        Err(error) => {
            let entry = make_log_entry(
                &id,
                started_at,
                method.as_str(),
                &path,
                LogFields {
                    protocol: Some(protocol_id.to_string()),
                    client_model: Some(client_model),
                    status_code: error.status,
                    outcome: "rejected".to_string(),
                    error_code: Some(error.code.clone()),
                    ..Default::default()
                },
            );
            emit_log(&state, &entry);
            return json_response(error.status, &json!({ "error": { "type": error.code, "message": error.message } }));
        }
    };

    // 上游格式：auto 表示跟随客户端。认证方式、端点改写与要不要转换都按它决定
    let upstream_format = if route.api_format.is_empty() || route.api_format == "auto" {
        protocol_id.to_string()
    } else {
        route.api_format.clone()
    };
    let Ok(adapter) = get_adapter(&upstream_format) else {
        return json_response(
            500,
            &json!({ "error": { "type": "ccproxy_internal", "message": format!("未知的上游格式：{}", route.api_format) } }),
        );
    };
    // 响应回写与用量按**客户端**格式算：转换后的响应已经是客户端格式了
    let client_adapter = get_adapter(&protocol_id).ok();
    let converting = convert_bridge::needs_conversion(&protocol_id, &upstream_format);

    // /v1/messages/count_tokens 且上游不是 Anthropic：这个端点转不了（拿不到对方的 tokenizer），
    // 本地估一个整数返回，不让这类请求打到不认它的上游
    if protocol_id == "anthropic" && is_count_tokens_path(&path) && upstream_format != "anthropic" {
        let tokens = estimate_input_tokens(payload.as_ref().unwrap_or(&Value::Null));
        let entry = make_log_entry(
            &id,
            started_at,
            method.as_str(),
            &path,
            LogFields {
                protocol: Some(protocol_id.to_string()),
                client_model: Some(client_model.clone()),
                provider_id: Some(route.provider_id.clone()),
                provider_name: Some(route.provider_name.clone()),
                real_model: Some(route.real_model.clone()),
                status_code: 200,
                outcome: "count_tokens_local".to_string(),
                ..Default::default()
            },
        );
        emit_log(&state, &entry);
        return json_response(200, &json!({ "input_tokens": tokens }));
    }

    // 请求体：先换 model，再按「客户端格式 → 上游格式」整段转换。
    // 不转换且 model 没变时保持原样字节透传
    let original_request = payload.clone().unwrap_or(Value::Null);
    let mut forward_body = body.clone();
    if let Some(payload) = payload.as_ref() {
        let mut next = payload.clone();
        adapter.rewrite_model(&mut next, &route.real_model);
        if converting {
            // Responses → Chat 之前先补历史：客户端只回传 tool_call id 时参数要靠它找回来
            convert_bridge::enrich_request(&protocol_id, &upstream_format, &mut next).await;
            match convert_bridge::convert_request(&protocol_id, &upstream_format, &next) {
                Ok(converted) => next = converted,
                Err(message) => {
                    let entry = make_log_entry(
                        &id,
                        started_at,
                        method.as_str(),
                        &path,
                        LogFields {
                            protocol: Some(protocol_id.to_string()),
                            client_model: Some(client_model.clone()),
                            provider_id: Some(route.provider_id.clone()),
                            provider_name: Some(route.provider_name.clone()),
                            real_model: Some(route.real_model.clone()),
                            status_code: 502,
                            outcome: "convert_failed".to_string(),
                            error_code: Some("convert_request_failed".to_string()),
                            error: Some(message.clone()),
                            ..Default::default()
                        },
                    );
                    emit_log(&state, &entry);
                    return json_response(
                        502,
                        &json!({ "error": { "type": "ccproxy_convert", "message": message } }),
                    );
                }
            }
        }
        if converting || payload.get("model").and_then(Value::as_str) != Some(route.real_model.as_str()) {
            if let Ok(encoded) = serde_json::to_vec(&next) {
                forward_body = Bytes::from(encoded);
            }
        }
    }

    // 出站头：清洗 → 剥离入站凭据 → 注入供应商真实凭据 → 重算 content-length
    let mut outbound_headers = sanitize_outbound_headers(&headers);
    strip_inbound_credentials(&mut outbound_headers);
    adapter.apply_auth(&mut outbound_headers, &route.token);
    if let Ok(value) = HeaderValue::from_str(&forward_body.len().to_string()) {
        outbound_headers.insert("content-length", value);
    }

    // 端点按上游格式改写：客户端说 Anthropic、上游说 Chat 时，路径要换成 chat 的端点
    let url = build_upstream_url(&route.base_url, &adapter.upstream_path(&path));

    // 响应回写：把上游回传的真实模型名换回 clientModel。请求体里连 model 都没有时不回写
    let rewriter = if client_model.is_empty() {
        None
    } else {
        client_adapter.map(|adapter| Arc::new(ResponseRewriter::new(adapter, &client_model)))
    };

    let build_entry: LogSink = {
        let state = state.clone();
        let id = id.clone();
        let method = method.as_str().to_string();
        let path = path.clone();
        let client_model = client_model.clone();
        let route = (
            route.match_kind.clone(),
            route.mapping.client_model.clone(),
            route.mapping.id.clone(),
            route.provider_id.clone(),
            route.provider_name.clone(),
            route.real_model.clone(),
        );
        Arc::new(move |outcome: ForwardOutcome, tokens: Option<Value>| {
            let entry = make_log_entry(
                &id,
                started_at,
                &method,
                &path,
                LogFields {
                    protocol: Some(protocol_id.to_string()),
                    client_model: Some(client_model.clone()),
                    match_kind: Some(route.0.clone()),
                    matched_rule: Some(route.1.clone()),
                    mapping_id: Some(route.2.clone()),
                    provider_id: Some(route.3.clone()),
                    provider_name: Some(route.4.clone()),
                    real_model: Some(route.5.clone()),
                    status_code: outcome.status_code,
                    outcome: "forwarded".to_string(),
                    error_code: None,
                    error: outcome.error,
                    cancelled: outcome.cancelled,
                    tokens,
                },
            );
            emit_log(&state, &entry);
        })
    };

    let upstream = match send_upstream(&state.http_client, &url, &method, outbound_headers, forward_body).await {
        Ok(upstream) => upstream,
        Err(message) => {
            build_entry(
                ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                None,
            );
            return json_response(
                502,
                &json!({ "error": { "type": "ccproxy_upstream", "message": format!("上游请求失败: {message}") } }),
            );
        }
    };

    let status = upstream.status().as_u16();
    let upstream_headers = upstream.headers().clone();

    // 上游是否按 SSE 回话：没标 text/event-stream 时要当心它压根没理会 stream 参数
    let upstream_is_sse = upstream_headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);

    // 转换方向下上游报错时，错误体也换一套形状（状态码保留）。这一步要赶在流式分支之前：
    // 错误体不是 SSE，交给状态机会被解成垃圾
    if converting && !(200..300).contains(&status) {
        let body_bytes = tokio::time::timeout(Duration::from_millis(NON_STREAM_TIMEOUT_MS), upstream.bytes())
            .await
            .ok()
            .and_then(|result| result.ok())
            .unwrap_or_default();
        let out = convert_bridge::error_body_for_client(&protocol_id, status, &body_bytes)
            .map(Bytes::from)
            .unwrap_or(body_bytes);
        build_entry(
            ForwardOutcome {
                status_code: status,
                error: Some(format!("上游返回 HTTP {status}")),
                cancelled: false,
            },
            None,
        );
        let mut builder = Response::builder().status(status);
        if let Some(headers_mut) = builder.headers_mut() {
            *headers_mut = passthrough_response_headers(&upstream_headers, true);
            headers_mut.insert("content-type", HeaderValue::from_static("application/json"));
            if let Ok(value) = HeaderValue::from_str(&out.len().to_string()) {
                headers_mut.insert("content-length", value);
            }
        }
        return builder.body(Body::from(out)).unwrap_or_else(|_| Response::new(Body::empty()));
    }

    // 转换过的响应体形状与上游不同，报错体的形状各家不一，硬转容易把失败原因转丢：
    // 上游报错时不转换，状态码与原始错误体一起透传
    let converting_body = converting && (200..300).contains(&status);

    if is_streaming {
        // 上游没按 stream 参数办事（content-type 不是 SSE）时先整块读回来：客户端等的是 SSE，
        // 把一整块 JSON 丢过去它只会当成空响应。先转成客户端格式，再展开成 SSE 生命周期
        if !upstream_is_sse {
            let collected =
                match tokio::time::timeout(Duration::from_millis(NON_STREAM_TIMEOUT_MS), upstream.bytes()).await {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        build_entry(
                            ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                            None,
                        );
                        return json_response(
                            502,
                            &json!({ "error": { "type": "ccproxy_upstream", "message": format!("上游请求失败: {message}") } }),
                        );
                    }
                    Err(_) => {
                        let message = format!("上游响应超过 {NON_STREAM_TIMEOUT_MS}ms");
                        build_entry(
                            ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                            None,
                        );
                        return json_response(502, &json!({ "error": { "type": "ccproxy_upstream", "message": message } }));
                    }
                };

            let parsed = serde_json::from_slice::<Value>(&collected).ok();
            let client_value = match (parsed, converting_body) {
                (Some(value), true) => {
                    match convert_bridge::convert_response(&protocol_id, &upstream_format, &original_request, value)
                        .await
                    {
                        Ok(converted) => Some(converted),
                        Err(message) => {
                            build_entry(
                                ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                                None,
                            );
                            return json_response(
                                502,
                                &json!({ "error": { "type": "ccproxy_convert", "message": message } }),
                            );
                        }
                    }
                }
                (value, _) => value,
            };

            // 先回写 model（展开器要从 message / response 里取它），再展开给客户端
            if let Some(value) = client_value.filter(|value| !convert_bridge::is_error_envelope(value)) {
                let encoded = Bytes::from(serde_json::to_vec(&value).unwrap_or_default());
                let rewritten = match &rewriter {
                    Some(rewriter) => rewriter.rewrite_body(&encoded),
                    None => encoded,
                };
                if let Ok(parsed) = serde_json::from_slice::<Value>(&rewritten) {
                    if let Ok(events) = convert_bridge::json_to_sse(&protocol_id, &parsed) {
                        let out: Vec<u8> = events.iter().flat_map(|item| item.to_vec()).collect();
                        let tokens = rewriter
                            .as_ref()
                            .and_then(|rewriter| rewriter.usage())
                            .map(|usage| usage.to_json());
                        build_entry(
                            ForwardOutcome { status_code: status, error: None, cancelled: false },
                            tokens,
                        );
                        let mut builder = Response::builder().status(status);
                        if let Some(headers_mut) = builder.headers_mut() {
                            headers_mut.insert("content-type", HeaderValue::from_static("text/event-stream"));
                            headers_mut.insert("cache-control", HeaderValue::from_static("no-cache"));
                            if let Ok(value) = HeaderValue::from_str(&out.len().to_string()) {
                                headers_mut.insert("content-length", value);
                            }
                        }
                        return builder.body(Body::from(out)).unwrap_or_else(|_| Response::new(Body::empty()));
                    }
                }
            }

            // 展不开（客户端协议没有展开器、或响应本身就是错误信封）就原样回：
            // 绝不伪造一个「成功但空」的 SSE，那是最难查的一种失败
            build_entry(ForwardOutcome { status_code: status, error: None, cancelled: false }, None);
            let mut builder = Response::builder().status(status);
            if let Some(headers_mut) = builder.headers_mut() {
                *headers_mut = passthrough_response_headers(&upstream_headers, false);
            }
            return builder.body(Body::from(collected)).unwrap_or_else(|_| Response::new(Body::empty()));
        }

        // 流式：边收边写，收尾（或客户端断开）时补埋点
        let hook: FinishHook = Arc::new(Mutex::new(Some({
            let build_entry = build_entry.clone();
            let rewriter = rewriter.clone();
            Box::new(move |outcome: ForwardOutcome| {
                let tokens = rewriter
                    .as_ref()
                    .and_then(|rewriter| rewriter.usage())
                    .map(|usage| usage.to_json());
                build_entry(outcome, tokens);
            }) as Box<dyn FnOnce(ForwardOutcome) + Send>
        })));
        // 需要的字节流：透传时是上游原始 SSE，转换时是上游 SSE 经状态机吐出的客户端格式 SSE
        let raw = upstream.bytes_stream();
        let source: UpstreamStream = if converting_body {
            let mapped = raw.map(|item| item.map_err(|error| convert_bridge::StreamError(error.to_string())));
            match convert_bridge::convert_stream(&protocol_id, &upstream_format, &original_request, mapped) {
                Ok(converted) => Box::pin(converted.map(|item| item.map_err(|error| error.to_string()))),
                Err(message) => {
                    let entry = make_log_entry(
                        &id,
                        started_at,
                        method.as_str(),
                        &path,
                        LogFields {
                            protocol: Some(protocol_id.to_string()),
                            client_model: Some(client_model.clone()),
                            provider_id: Some(route.provider_id.clone()),
                            provider_name: Some(route.provider_name.clone()),
                            real_model: Some(route.real_model.clone()),
                            status_code: 502,
                            outcome: "convert_failed".to_string(),
                            error_code: Some("convert_stream_failed".to_string()),
                            error: Some(message.clone()),
                            ..Default::default()
                        },
                    );
                    emit_log(&state, &entry);
                    return json_response(
                        502,
                        &json!({ "error": { "type": "ccproxy_convert", "message": message } }),
                    );
                }
            }
        } else {
            Box::pin(raw.map(|item| item.map_err(|error| error.to_string())))
        };
        let body = Body::from_stream(make_stream(source, rewriter.clone(), hook, status));
        let mut builder = Response::builder().status(status);
        if let Some(headers_mut) = builder.headers_mut() {
            *headers_mut = passthrough_response_headers(&upstream_headers, rewriter.is_some());
        }
        return builder.body(body).unwrap_or_else(|_| Response::new(Body::empty()));
    }

    // 非流式：整体缓冲（回写与不回写都缓冲，埋点顺序与流式一致），带整体超时
    let collected = match tokio::time::timeout(Duration::from_millis(NON_STREAM_TIMEOUT_MS), upstream.bytes()).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let message = error.to_string();
            build_entry(
                ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                None,
            );
            return json_response(
                502,
                &json!({ "error": { "type": "ccproxy_upstream", "message": format!("上游请求失败: {message}") } }),
            );
        }
        Err(_) => {
            let message = format!("非流式响应超过 {NON_STREAM_TIMEOUT_MS}ms");
            build_entry(
                ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                None,
            );
            return json_response(
                502,
                &json!({ "error": { "type": "ccproxy_upstream", "message": format!("上游请求失败: {message}") } }),
            );
        }
    };

    // 非流式：转换后的响应体已经是客户端格式，model 回写与用量提取都按客户端格式做
    let out = if converting_body {
        match serde_json::from_slice::<Value>(&collected) {
            Ok(parsed) => {
                match convert_bridge::convert_response(&protocol_id, &upstream_format, &original_request, parsed)
                    .await
                {
                    Ok(converted) => {
                        let bytes = Bytes::from(serde_json::to_vec(&converted).unwrap_or_default());
                        match &rewriter {
                            Some(rewriter) => rewriter.rewrite_body(&bytes),
                            None => bytes,
                        }
                    }
                    Err(message) => {
                        build_entry(
                            ForwardOutcome {
                                status_code: 502,
                                error: Some(message.clone()),
                                cancelled: false,
                            },
                            None,
                        );
                        return json_response(
                            502,
                            &json!({ "error": { "type": "ccproxy_convert", "message": message } }),
                        );
                    }
                }
            }
            Err(error) => {
                let text = String::from_utf8_lossy(&collected);
                let looks_like_sse = ["data:", "event:", "id:", "retry:"]
                    .iter()
                    .any(|prefix| text.trim_start_matches('\u{feff}').trim_start().starts_with(prefix));
                if looks_like_sse {
                    // 上游回了 SSE，而客户端要的是整块（转不了）：原样透传比编一个 502 诚实，
                    // 客户端至少能自己看出上游回的是什么
                    build_entry(ForwardOutcome { status_code: status, error: None, cancelled: false }, None);
                    let mut builder = Response::builder().status(status);
                    if let Some(headers_mut) = builder.headers_mut() {
                        *headers_mut = passthrough_response_headers(&upstream_headers, false);
                    }
                    return builder.body(Body::from(collected)).unwrap_or_else(|_| Response::new(Body::empty()));
                }
                let message = format!("上游响应不是合法 JSON，无法转换：{error}");
                build_entry(
                    ForwardOutcome { status_code: 502, error: Some(message.clone()), cancelled: false },
                    None,
                );
                return json_response(502, &json!({ "error": { "type": "ccproxy_convert", "message": message } }));
            }
        }
    } else {
        match &rewriter {
            Some(rewriter) => rewriter.rewrite_body(&collected),
            None => collected,
        }
    };
    let tokens = rewriter
        .as_ref()
        .and_then(|rewriter| rewriter.usage())
        .map(|usage| usage.to_json());
    build_entry(ForwardOutcome { status_code: status, error: None, cancelled: false }, tokens);

    // 响应体被改过（回写 model 或做过转换）时 content-length 都要按新的算
    let rebuilt_length = rewriter.is_some() || converting_body;
    let mut builder = Response::builder().status(status);
    if let Some(headers_mut) = builder.headers_mut() {
        *headers_mut = passthrough_response_headers(&upstream_headers, rebuilt_length);
        if rebuilt_length {
            if let Ok(value) = HeaderValue::from_str(&out.len().to_string()) {
                headers_mut.insert("content-length", value);
            }
        }
    }
    builder.body(Body::from(out)).unwrap_or_else(|_| Response::new(Body::empty()))
}


