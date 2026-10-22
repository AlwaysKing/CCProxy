// 端到端集成测试：mock 上游 + 真代理进程内服务，验证识别→查表→改写→转发→透传全链路与错误场景

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Response};
use axum::routing::any;
use axum::Router;
use futures_util::StreamExt;
use serde_json::{json, Value};

use ccproxy_lib::state::AppState;

use common::{config_with_port, test_state, TempDir};

mod common;

// 一次被 mock 上游看到的请求
#[derive(Clone)]
struct Captured {
    url: String,
    headers: HeaderMap,
    body: Value,
}

type Sink = Arc<Mutex<Vec<Captured>>>;

fn sink() -> Sink {
    Arc::new(Mutex::new(Vec::new()))
}

fn last_captured(sink: &Sink) -> Captured {
    sink.lock().unwrap().last().cloned().expect("上游应收到请求")
}

async fn start_mock(app: Router) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    (port, tx)
}

// 起一个会记录入站请求、并按 handler 回响应的 mock 上游
async fn mock_upstream<F>(sink: Sink, handler: F) -> (u16, tokio::sync::oneshot::Sender<()>)
where
    F: Fn(&Captured) -> Response<Body> + Send + Sync + Clone + 'static,
{
    let app = Router::new().fallback(any(move |request: axum::extract::Request| {
        let sink = sink.clone();
        let handler = handler.clone();
        async move {
            let url = request
                .uri()
                .path_and_query()
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            let headers = request.headers().clone();
            let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024 * 1024).await.unwrap_or_default();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let captured = Captured { url, headers, body };
            if let Ok(mut entries) = sink.lock() {
                entries.push(captured.clone());
            }
            handler(&captured)
        }
    }));
    start_mock(app).await
}

async fn start_proxy(state: &Arc<AppState>) -> u16 {
    state.proxy.start().await.expect("代理应能监听")
}

fn claude_body(model: &str, stream: bool) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": model,
        "max_tokens": 1024,
        "system": "be brief",
        "stream": stream,
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn sse_response(body: String) -> Response<Body> {
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

// 一个 anthropic 供应商配置，baseUrl 指向 mock 上游
fn anthropic_config(mock_port: u16, client_model: &str) -> Value {
    json!({
        "settings": { "clientToken": "sk-client-token" },
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [{
                "id": "p1", "name": "mock上游", "enabled": true,
                "baseUrl": format!("http://127.0.0.1:{mock_port}"),
                "token": "sk-real-token",
                "models": [{ "name": "glm-4.6" }]
            }],
            "mappings": [
                { "id": "m1", "enabled": true, "clientModel": client_model, "providerId": "p1", "realModel": "glm-4.6" }
            ]
        }]
    })
}

#[tokio::test]
async fn end_to_end_anthropic_detect_token_rewrite_and_sse_passthrough() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        sse_response(
            [
                "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ]
            .concat(),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "claude-sonnet-4-*"), 0), &dir);
    let port = start_proxy(&state).await;

    // 模拟 Claude Code 的请求：x-api-key 带假 key
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-fake-from-client")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .body(claude_body("claude-sonnet-4-20250514", true))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    assert!(text.contains("message_start"));

    let seen = last_captured(&captured);
    assert_eq!(seen.url, "/v1/messages");
    assert_eq!(seen.headers["x-api-key"], "sk-real-token");
    assert!(seen.headers.get("authorization").is_none());
    assert_eq!(seen.body["model"], json!("glm-4.6"));
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_response_model_rewritten_back() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |seen| {
        let want_stream = seen.body["stream"] == json!(true);
        if want_stream {
            // message_start 事件里 model 的 JSON 会被 HTTP 层切成两块，验证事件拼装不依赖 chunk 边界
            let text = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-4.6\",\"role\":\"assistant\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
            sse_response(text.to_string())
        } else {
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "id": "msg_1", "type": "message", "model": "glm-4.6", "content": [] })).unwrap(),
                ))
                .unwrap()
        }
    })
    .await;

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "cnsinda-glm-4.6"), 0), &dir);
    let port = start_proxy(&state).await;
    let client = reqwest::Client::new();

    let non_stream = client
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-fake")
        .header("anthropic-version", "2023-06-01")
        .body(claude_body("cnsinda-glm-4.6", false))
        .send()
        .await
        .unwrap();
    assert_eq!(non_stream.status(), 200);
    let content_length = non_stream.headers()["content-length"].to_str().unwrap().to_string();
    let text = non_stream.text().await.unwrap();
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["model"], json!("cnsinda-glm-4.6"));
    assert_eq!(content_length.parse::<usize>().unwrap(), text.len());

    let stream = client
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-fake")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .body(claude_body("cnsinda-glm-4.6", true))
        .send()
        .await
        .unwrap();
    let text = stream.text().await.unwrap();
    assert!(text.contains("\"model\":\"cnsinda-glm-4.6\""), "实际：{text}");
    assert!(text.contains("message_stop"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_openai_uses_bearer() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "model": "deepseek-chat", "choices": [] })).unwrap(),
            ))
            .unwrap()
    })
    .await;

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(
        config_with_port(
            json!({
                "protocols": [{
                    "id": "openai", "name": "OpenAI", "enabled": true,
                    "providers": [{
                        "id": "q1", "name": "mock上游", "enabled": true,
                        "baseUrl": format!("http://127.0.0.1:{mock_port}/v1"),
                        "token": "sk-real-openai",
                        "models": [{ "name": "deepseek-chat" }]
                    }],
                    "mappings": [
                        { "id": "m1", "enabled": true, "clientModel": "gpt-4o", "providerId": "q1", "realModel": "deepseek-chat" }
                    ]
                }]
            }),
            0,
        ),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer sk-fake")
        .body(serde_json::to_vec(&json!({ "model": "gpt-4o", "messages": [] })).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["model"], json!("gpt-4o"));

    let seen = last_captured(&captured);
    assert_eq!(seen.url, "/v1/chat/completions");
    assert_eq!(seen.headers["authorization"], "Bearer sk-real-openai");
    assert!(seen.headers.get("x-api-key").is_none());
    assert_eq!(seen.body["model"], json!("deepseek-chat"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

// Codex 走 responses 线格式：用量在流式终止事件里、模型名在 response.model 上，
// 端到端要能记进用量聚合、也要把真实模型名换回客户端名
#[tokio::test]
async fn end_to_end_responses_stream_records_usage() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        sse_response(
            [
                "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-vl-flash\"},\"sequence_number\":0}\n\n",
                "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-vl-flash\",\"status\":\"completed\",\"usage\":{\"input_tokens\":120,\"input_tokens_details\":{\"cached_tokens\":64},\"output_tokens\":45,\"output_tokens_details\":{\"reasoning_tokens\":12},\"total_tokens\":165}}}\n\n",
            ]
            .concat(),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(
        config_with_port(
            json!({
                "protocols": [{
                    "id": "openai", "name": "OpenAI", "enabled": true,
                    "providers": [{
                        "id": "q1", "name": "mock上游", "enabled": true,
                        "baseUrl": format!("http://127.0.0.1:{mock_port}/v1"),
                        "token": "sk-real-openai",
                        "models": [{ "name": "deepseek-vl-flash" }]
                    }],
                    "mappings": [
                        { "id": "m1", "enabled": true, "clientModel": "deepseek-flash", "providerId": "q1", "realModel": "deepseek-vl-flash" }
                    ]
                }]
            }),
            0,
        ),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer sk-fake")
        .header("accept", "text/event-stream")
        .body(serde_json::to_vec(&json!({ "model": "deepseek-flash", "stream": true, "input": [] })).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(text.contains("response.completed"));
    let seen = last_captured(&captured);
    assert_eq!(seen.url, "/v1/responses");
    // 请求侧：发给上游的是真实模型名；响应侧：客户端看到的必须是映射名
    assert_eq!(seen.body["model"], json!("deepseek-vl-flash"));
    assert!(text.contains("\"model\":\"deepseek-flash\""), "实际：{text}");
    assert!(!text.contains("deepseek-vl-flash"), "真实模型名不该回给客户端：{text}");

    let usage = state.request_log.usage(&json!({ "days": 1 }));
    assert_eq!(usage["totals"]["requests"], json!(1));
    assert_eq!(usage["totals"]["input"], json!(120));
    assert_eq!(usage["totals"]["output"], json!(45));
    // responses 的 input_tokens 含缓存命中，缓存只作为明细一路传到用量聚合里
    assert_eq!(usage["totals"]["cacheRead"], json!(64));
    assert_eq!(usage["byModel"][0]["key"], json!("deepseek-vl-flash"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_responses_stream_without_content_type_records_usage() {
    // 网关的流式回复只带 chunked、不带 content-type（本机 company 那条线上就是这样），
    // 只看响应头的话代理会把整段 SSE 当 JSON 缓冲：分块没了、用量和模型回写也一起丢
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        Response::builder()
            .status(200)
            .body(Body::from(
                [
                    "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-vl-flash\"},\"sequence_number\":0}\n\n",
                    "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-vl-flash\",\"status\":\"completed\",\"usage\":{\"input_tokens\":120,\"input_tokens_details\":{\"cached_tokens\":64},\"output_tokens\":45,\"total_tokens\":165}}}\n\n",
                ]
                .concat(),
            ))
            .unwrap()
    })
    .await;

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(
        config_with_port(
            json!({
                "protocols": [{
                    "id": "openai", "name": "OpenAI", "enabled": true,
                    "providers": [{
                        "id": "q1", "name": "mock上游", "enabled": true,
                        "baseUrl": format!("http://127.0.0.1:{mock_port}/v1"),
                        "token": "sk-real-openai",
                        "models": [{ "name": "deepseek-vl-flash" }]
                    }],
                    "mappings": [
                        { "id": "m1", "enabled": true, "clientModel": "deepseek-flash", "providerId": "q1", "realModel": "deepseek-vl-flash" }
                    ]
                }]
            }),
            0,
        ),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer sk-fake")
        .header("accept", "text/event-stream")
        .body(serde_json::to_vec(&json!({ "model": "deepseek-flash", "stream": true, "input": [] })).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    // 上游没写内容类型，代理认出来之后要补一个，客户端才知道这是 SSE
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    assert!(text.contains("response.completed"));
    assert!(text.contains("\"model\":\"deepseek-flash\""), "实际：{text}");
    assert!(!text.contains("deepseek-vl-flash"), "真实模型名不该回给客户端：{text}");

    let usage = state.request_log.usage(&json!({ "days": 1 }));
    assert_eq!(usage["totals"]["requests"], json!(1));
    assert_eq!(usage["totals"]["input"], json!(120));
    assert_eq!(usage["totals"]["output"], json!(45));
    assert_eq!(usage["totals"]["cacheRead"], json!(64));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_mapping_miss_is_404() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| Response::new(Body::from("nope"))).await;
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "claude-only"), 0), &dir);
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("x-api-key", "sk-fake")
        .body(claude_body("something-else", false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(payload["error"]["type"], json!("mapping_not_found"));
    assert!(payload["error"]["message"].as_str().unwrap().contains("something-else"));
    // 上游一次都不该被打到
    assert!(captured.lock().unwrap().is_empty());

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_unknown_path_is_400() {
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(1, "claude-*"), 0), &dir);
    let port = start_proxy(&state).await;

    // 带齐 Anthropic 的头与请求体特征：协议只看路径，这些特征救不回一条不认识的路径
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/foo"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("x-api-key", "sk-fake")
        .body(claude_body("claude-sonnet-4-20250514", false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(payload["error"]["type"], json!("ccproxy_detect"));
    // 文案要把认识的路径列出来，用户照着改 baseURL 就能通
    let message = payload["error"]["message"].as_str().unwrap();
    assert!(message.contains("/foo"));
    assert!(message.contains("/v1/messages"));
    assert!(message.contains("/v1/responses"));
    assert!(payload["error"].get("detail").is_none());

    state.proxy.stop().await;
}

#[tokio::test]
async fn end_to_end_models_endpoint_is_intercepted() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| Response::new(Body::from("nope"))).await;
    let dir = TempDir::new("ccproxy-e2e");
    let config = json!({
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "providers": [{ "id": "p1", "enabled": true, "baseUrl": format!("http://127.0.0.1:{mock_port}"), "token": "t", "models": [] }],
            "mappings": [
                { "id": "m1", "enabled": true, "clientModel": "claude-opus-4", "providerId": "p1", "realModel": "glm-4.6" },
                { "id": "m2", "enabled": true, "clientModel": "*", "providerId": "p1", "realModel": "glm-4.6" }
            ]
        }]
    });
    let state = test_state(config_with_port(config, 0), &dir);
    let port = start_proxy(&state).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("http://127.0.0.1:{port}/v1/models"))
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    // 通配与兜底不是真实模型名，不列入列表
    assert_eq!(payload["data"].as_array().unwrap().len(), 1);
    assert_eq!(payload["data"][0]["id"], json!("claude-opus-4"));
    assert_eq!(payload["data"][0]["type"], json!("model"));
    assert!(captured.lock().unwrap().is_empty(), "模型列表不该透传上游");

    // openai 客户端拿到 openai 格式
    let response = client.get(format!("http://127.0.0.1:{port}/v1/models")).send().await.unwrap();
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(payload["object"], json!("list"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_unreachable_upstream_is_502() {
    let dir = TempDir::new("ccproxy-e2e");
    // 指向一个没人监听的端口
    let state = test_state(config_with_port(anthropic_config(9, "claude-*"), 0), &dir);
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("x-api-key", "sk-fake")
        .body(claude_body("claude-sonnet-4", false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(payload["error"]["type"], json!("ccproxy_upstream"));

    state.proxy.stop().await;
}

#[tokio::test]
async fn end_to_end_oversized_body_is_413_json() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| Response::new(Body::from("nope"))).await;
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "claude-*"), 0), &dir);
    // 把上限压到 1KB，免得测试要真造 64MB
    state.set_max_body_bytes(1024);
    let port = start_proxy(&state).await;

    let big = "x".repeat(4096);
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("x-api-key", "sk-fake")
        .body(serde_json::to_vec(&json!({ "model": "claude-sonnet-4", "max_tokens": 1, "system": big })).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let payload: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(payload["error"]["type"], json!("ccproxy_body"));
    assert!(captured.lock().unwrap().is_empty());
    // 413 也要留下可判读的日志
    let entry = wait_for_entry(&state, |entry| entry["outcome"] == json!("read_failed")).await;
    assert_eq!(entry["errorCode"], json!("body_too_large"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_streaming_usage_lands_in_log_entry() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        sse_response(
            [
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-4.6\",\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":257}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ]
            .concat(),
        )
    })
    .await;
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "cnsinda-glm-4.6"), 0), &dir);
    let port = start_proxy(&state).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("x-api-key", "sk-fake")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .body(claude_body("cnsinda-glm-4.6", true))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let _ = response.text().await.unwrap();

    let entry = wait_for_entry(&state, |entry| entry["outcome"] == json!("forwarded")).await;
    assert_eq!(entry["protocol"], json!("anthropic"));
    assert_eq!(entry["clientModel"], json!("cnsinda-glm-4.6"));
    assert_eq!(entry["matchKind"], json!("exact"));
    assert_eq!(entry["providerName"], json!("mock上游"));
    assert_eq!(entry["realModel"], json!("glm-4.6"));
    assert_eq!(entry["statusCode"], json!(200));
    assert_eq!(entry["cancelled"], json!(false));
    // message_delta 的累计值不能被翻倍
    assert_eq!(entry["tokens"]["input"], json!(100));
    assert_eq!(entry["tokens"]["output"], json!(257));
    // 用量也落了盘
    assert_eq!(state.request_log.usage(&json!({}))["totals"]["requests"], json!(1));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn end_to_end_client_abort_is_recorded_as_cancelled() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        // 上游写完一个事件就不收尾：这样客户端断开时流确实还在进行中
        let first = futures_util::stream::once(async {
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
            ))
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(first.chain(futures_util::stream::pending())))
            .unwrap()
    })
    .await;
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(mock_port, "cnsinda-glm-4.6"), 0), &dir);
    let port = start_proxy(&state).await;

    // 用裸 socket 发请求，读到一半直接关掉：这就是「客户端提前断开」。
    // 阻塞 socket 必须丢到 blocking 线程上，否则会把 async 运行时的线程占住，代理永远处理不到这个请求
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        use std::time::Duration;
        let body = claude_body("cnsinda-glm-4.6", true);
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let request = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nx-api-key: sk-fake\r\nanthropic-version: 2023-06-01\r\naccept: text/event-stream\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        let mut buffer = [0u8; 64];
        let _ = stream.read(&mut buffer);
        drop(stream);
    })
    .await
    .unwrap();

    let entry = wait_for_entry(&state, |entry| entry["cancelled"] == json!(true)).await;
    assert_eq!(entry["outcome"], json!("forwarded"));
    assert_eq!(entry["statusCode"], json!(502));
    assert_eq!(entry["error"], json!("客户端提前断开"));
    // 取消不算上游失败
    assert_eq!(state.request_log.usage(&json!({}))["totals"]["cancelled"], json!(1));
    assert_eq!(state.request_log.usage(&json!({}))["totals"]["failed"], json!(0));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn start_is_idempotent_and_stop_releases_the_port() {
    // 先占一个空闲端口再放掉，拿它当固定端口用：配置里写 0 的话每次 start 都会换一个端口
    let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let fixed = probe.local_addr().unwrap().port();
    drop(probe);

    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(1, "claude-*"), fixed), &dir);

    // 并发 start 复用同一次监听
    let first = state.clone();
    let second = state.clone();
    let (a, b) = tokio::join!(first.proxy.start(), second.proxy.start());
    assert_eq!(a.unwrap(), fixed);
    assert_eq!(b.unwrap(), fixed);

    // 再 start 还是同一个端口，不会各起一个
    assert_eq!(state.proxy.start().await.unwrap(), fixed);

    state.proxy.stop().await;
    // 停掉之后端口真的释放了，可以重新绑上
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", fixed)).await;
    assert!(listener.is_ok(), "stop 之后端口应已释放");
    drop(listener);

    // 停了还能再起，还是同一个端口
    assert_eq!(state.proxy.start().await.unwrap(), fixed);
    state.proxy.stop().await;
}

#[tokio::test]
async fn stop_after_start_leaves_no_listener() {
    let dir = TempDir::new("ccproxy-e2e");
    let state = test_state(config_with_port(anthropic_config(1, "claude-*"), 0), &dir);

    let port = state.proxy.start().await.unwrap();
    state.proxy.stop().await;
    assert!(state.proxy.running_port().is_none());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await;
    assert!(listener.is_ok());
}

// 等一条满足条件的日志 entry 出现（埋点在响应收尾之后落，会晚于客户端拿到响应）
async fn wait_for_entry<F>(state: &Arc<AppState>, predicate: F) -> Value
where
    F: Fn(&Value) -> bool,
{
    for _ in 0..200 {
        if let Some(entry) = state.request_log.list().into_iter().find(|entry| predicate(entry)) {
            return entry;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("没有等到预期的日志 entry：{:?}", state.request_log.list());
}

