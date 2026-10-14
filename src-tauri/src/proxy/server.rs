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
use crate::proxy::detector::{detect_protocol, parse_body};
use crate::proxy::models::{build_models_response, match_models_request, resolve_client_protocol};
use crate::proxy::router::{resolve_routing, Route};
use crate::proxy::upstream::{
    build_upstream_url, passthrough_response_headers, sanitize_outbound_headers, send_upstream, NON_STREAM_TIMEOUT_MS,
    STREAM_IDLE_TIMEOUT_MS,
};
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
    detection: Option<Value>,
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
        "tokens": fields.tokens,
        "detection": fields.detection
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

type UpstreamStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

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

// 流式转发：按 SSE 事件边界（\n\n）凑齐一个事件改写一个，空闲超时按块重置
fn make_stream(
    upstream: reqwest::Response,
    rewriter: Option<Arc<ResponseRewriter>>,
    hook: FinishHook,
    status: u16,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let state = StreamState {
        upstream: Box::pin(upstream.bytes_stream()),
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
                Ok(Some(Err(error))) => {
                    state.finished = true;
                    let message = error.to_string();
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

    let detection = detect_protocol(&path, &headers, &body);
    let detection_json = json!({
        "scores": { "anthropic": detection.scores.anthropic, "openai": detection.scores.openai },
        "reasons": detection.reasons
    });
    let Some(protocol_id) = detection.protocol.clone() else {
        let entry = make_log_entry(
            &id,
            started_at,
            method.as_str(),
            &path,
            LogFields {
                status_code: 400,
                outcome: "detect_failed".to_string(),
                detection: Some(detection_json.clone()),
                ..Default::default()
            },
        );
        emit_log(&state, &entry);
        return json_response(
            400,
            &json!({
                "error": {
                    "type": "ccproxy_detect",
                    "message": "无法识别请求所属协议",
                    "detail": detection_json
                }
            }),
        );
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
        resolve_routing(&config, &protocol_id, &client_model)
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
                    protocol: Some(protocol_id),
                    client_model: Some(client_model),
                    status_code: error.status,
                    outcome: "rejected".to_string(),
                    error_code: Some(error.code.clone()),
                    detection: Some(detection_json),
                    ..Default::default()
                },
            );
            emit_log(&state, &entry);
            return json_response(error.status, &json!({ "error": { "type": error.code, "message": error.message } }));
        }
    };

    let Ok(adapter) = get_adapter(&protocol_id) else {
        return json_response(500, &json!({ "error": { "type": "ccproxy_internal", "message": "未知协议" } }));
    };

    // 改写 model 字段；只有真的变了才重新序列化，没变就原样字节透传
    let mut forward_body = body.clone();
    if let Some(mut payload) = payload {
        if payload.get("model").and_then(Value::as_str) != Some(route.real_model.as_str()) {
            adapter.rewrite_model(&mut payload, &route.real_model);
            if let Ok(encoded) = serde_json::to_vec(&payload) {
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

    let url = build_upstream_url(&route.base_url, &path);

    // 响应回写：把上游回传的真实模型名换回 clientModel。请求体里连 model 都没有时不回写
    let rewriter = if client_model.is_empty() {
        None
    } else {
        Some(Arc::new(ResponseRewriter::new(adapter, &client_model)))
    };

    let build_entry: LogSink = {
        let state = state.clone();
        let id = id.clone();
        let method = method.as_str().to_string();
        let path = path.clone();
        let protocol_id = protocol_id.clone();
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
                    protocol: Some(protocol_id.clone()),
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
                    detection: Some(detection_json.clone()),
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

    if is_streaming {
        // 流式：边收边写，收尾（或客户端断开）时补埋点
        let hook: FinishHook = Arc::new(Mutex::new(Some({
            let build_entry = build_entry.clone();
            let rewriter = rewriter.clone();
            Box::new(move |outcome: ForwardOutcome| {
                let tokens = rewriter
                    .as_ref()
                    .and_then(|rewriter| rewriter.usage())
                    .map(|usage| json!({ "input": usage.input, "output": usage.output }));
                build_entry(outcome, tokens);
            }) as Box<dyn FnOnce(ForwardOutcome) + Send>
        })));
        let body = Body::from_stream(make_stream(upstream, rewriter.clone(), hook, status));
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

    let out = match &rewriter {
        Some(rewriter) => rewriter.rewrite_body(&collected),
        None => collected,
    };
    let tokens = rewriter
        .as_ref()
        .and_then(|rewriter| rewriter.usage())
        .map(|usage| json!({ "input": usage.input, "output": usage.output }));
    build_entry(ForwardOutcome { status_code: status, error: None, cancelled: false }, tokens);

    let mut builder = Response::builder().status(status);
    if let Some(headers_mut) = builder.headers_mut() {
        *headers_mut = passthrough_response_headers(&upstream_headers, rewriter.is_some());
        if rewriter.is_some() {
            if let Ok(value) = HeaderValue::from_str(&out.len().to_string()) {
                headers_mut.insert("content-length", value);
            }
        }
    }
    builder.body(Body::from(out)).unwrap_or_else(|_| Response::new(Body::empty()))
}


