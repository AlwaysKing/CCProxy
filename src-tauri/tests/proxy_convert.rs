// 协议转换的端到端测试：mock 上游 + 真代理，验证「客户端格式 → 供应商格式」的
// 端点改写、认证注入、请求体转换、响应体回转，以及转换不了时的失败方式。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, Response};
use axum::routing::any;
use axum::Router;
use serde_json::{json, Value};

use ccproxy_lib::state::AppState;

use common::{config_with_port, entry_value, test_state, TempDir};

mod common;

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

fn json_response(status: u16, payload: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

fn sse_response(body: String) -> Response<Body> {
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

// 指定「客户端协议 + 客户端模型名 + 上游格式」的配置，baseUrl 指向 mock 上游
fn config_for(mock_port: u16, protocol_id: &str, client_model: &str, api_format: &str) -> Value {
    json!({
        "settings": { "clientToken": "sk-client-token" },
        "protocols": [{
            "id": protocol_id, "enabled": true,
            "providers": [{
                "id": "p1", "name": "mock上游", "enabled": true,
                "baseUrl": format!("http://127.0.0.1:{mock_port}"),
                "token": "sk-real-token",
                "apiFormat": api_format,
                "models": [{ "name": "glm-4.6" }]
            }],
            "mappings": [
                { "id": "m1", "enabled": true, "clientModel": client_model, "providerId": "p1", "realModel": "glm-4.6" }
            ]
        }]
    })
}

fn claude_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model,
        "max_tokens": 1024,
        "system": "be brief",
        "stream": stream,
        "messages": [{ "role": "user", "content": "hi" }]
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn anthropic_client_uses_an_openai_chat_upstream() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        json_response(
            200,
            json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "model": "glm-4.6",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 11, "completion_tokens": 3, "total_tokens": 14 }
            }),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-fake-from-client")
        .header("anthropic-version", "2023-06-01")
        .json(&claude_body("claude-sonnet-4-20250514", false))
        .send()
        .await
        .unwrap();

    // 上游收到的是 Chat 格式：端点换成了 /v1/chat/completions，认证换成 Bearer
    let seen = captured.lock().unwrap().last().cloned().expect("上游应收到请求");
    assert_eq!(seen.url, "/v1/chat/completions");
    assert_eq!(seen.headers["authorization"], "Bearer sk-real-token");
    assert!(seen.headers.get("x-api-key").is_none());
    assert_eq!(seen.body["model"], json!("glm-4.6"));
    // system 提成了首条 system 消息，Anthropic 的 messages 结构没被原样发出去
    assert_eq!(seen.body["messages"][0]["role"], json!("system"));
    assert_eq!(seen.body["messages"][1]["content"], json!("hi"));
    assert!(seen.body.get("system").is_none());

    // 客户端拿到的是 Anthropic 形状，且 model 回写成客户端模型名
    assert_eq!(response.status(), 200);
    let payload: Value = response.json().await.unwrap();
    assert_eq!(payload["type"], json!("message"));
    assert_eq!(payload["model"], json!("claude-sonnet-4-20250514"));
    assert_eq!(payload["content"][0]["type"], json!("text"));
    assert_eq!(payload["content"][0]["text"], json!("hello"));
    assert_eq!(payload["stop_reason"], json!("end_turn"));
    // 用量从转换后的响应体提出来（Anthropic 口径）
    assert_eq!(payload["usage"]["input_tokens"], json!(11));
    assert_eq!(payload["usage"]["output_tokens"], json!(3));

    // 日志里带上了 token 统计
    let entry = entry_value(&state, 0);
    assert_eq!(entry["tokens"]["input"], json!(11));
    assert_eq!(entry["tokens"]["output"], json!(3));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn responses_client_uses_an_anthropic_upstream() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        json_response(
            200,
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "glm-4.6",
                "content": [{ "type": "text", "text": "hello" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 9, "output_tokens": 2 }
            }),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "openai_responses", "gpt-5-codex", "anthropic"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header("content-type", "application/json")
        .json(&json!({
            "model": "gpt-5-codex",
            "instructions": "be brief",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }]
        }))
        .send()
        .await
        .unwrap();

    // 上游收到的是 Anthropic 格式：端点、认证与必填的 max_tokens
    let seen = captured.lock().unwrap().last().cloned().expect("上游应收到请求");
    assert_eq!(seen.url, "/v1/messages");
    assert_eq!(seen.headers["x-api-key"], "sk-real-token");
    // Codex 不会带 anthropic-version，代理要补上，否则上游会拒
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");
    assert!(seen.body.get("instructions").is_none());
    assert!(seen.body["max_tokens"].is_number());
    assert_eq!(seen.body["model"], json!("glm-4.6"));

    // 客户端拿到的是 Responses 形状
    assert_eq!(response.status(), 200);
    let payload: Value = response.json().await.unwrap();
    assert_eq!(payload["object"], json!("response"));
    assert_eq!(payload["model"], json!("gpt-5-codex"));
    let output = payload["output"].as_array().expect("output 应是数组");
    assert_eq!(output[0]["type"], json!("message"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn anthropic_stream_is_converted_from_chat_chunks() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        sse_response(
            [
                "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hel\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n",
            ]
            .concat(),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .json(&claude_body("claude-sonnet-4-20250514", true))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    // 客户端拿到的是 Anthropic 的 SSE 生命周期，而不是原样的 chat chunk
    assert!(text.contains("event: message_start"), "实际：{text}");
    assert!(text.contains("content_block_delta"), "实际：{text}");
    assert!(text.contains("message_stop"), "实际：{text}");
    assert!(text.contains("hel") && text.contains("lo"), "实际：{text}");
    // 增量文本拼起来是对的
    assert!(!text.contains("chat.completion.chunk"));

    let seen = captured.lock().unwrap().last().cloned().expect("上游应收到请求");
    assert_eq!(seen.url, "/v1/chat/completions");
    assert_eq!(seen.body["stream"], json!(true));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn count_tokens_is_answered_locally_when_the_upstream_is_not_anthropic() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| json_response(200, json!({}))).await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages/count_tokens"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&claude_body("claude-sonnet-4-20250514", false))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let payload: Value = response.json().await.unwrap();
    assert!(payload["input_tokens"].as_i64().unwrap() > 0);
    // 这类请求不该打到上游：上游不是 Anthropic，它没有这个端点
    assert!(captured.lock().unwrap().is_empty(), "count_tokens 不该转发上游");

    let entry = entry_value(&state, 0);
    assert_eq!(entry["outcome"], json!("count_tokens_local"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn unsupported_conversion_fails_loudly_instead_of_forwarding_a_foreign_body() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| json_response(200, json!({}))).await;

    let dir = TempDir::new("ccproxy-convert");
    // 客户端走 chat，上游却是 anthropic 格式：这一对转换还没实现
    let state = test_state(
        config_with_port(config_for(mock_port, "openai_chat", "gpt-4o", "anthropic"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header("content-type", "application/json")
        .json(&json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 502);
    let payload: Value = response.json().await.unwrap();
    assert_eq!(payload["error"]["type"], json!("ccproxy_convert"));
    assert!(payload["error"]["message"].as_str().unwrap().contains("尚未实现"));
    // 关键：不能把 Chat 的请求体原样发给只认 Anthropic 的上游
    assert!(captured.lock().unwrap().is_empty(), "不该转发出去");

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn a_json_body_is_expanded_into_sse_when_the_upstream_ignores_stream() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        // 上游忽略了 stream:true，回了一整块 JSON（content-type 也是 json）
        json_response(
            200,
            json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "model": "glm-4.6",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 6, "completion_tokens": 2 }
            }),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .json(&claude_body("claude-sonnet-4-20250514", true))
        .send()
        .await
        .unwrap();

    // 客户端等的是 SSE，不能把一整块 JSON 丢回去（它只会当成空响应）
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    for needle in ["event: message_start", "content_block_delta", "message_stop", "hello"] {
        assert!(text.contains(needle), "缺少 {needle}：{text}");
    }
    // model 回写成客户端模型名（展开器从 message 里取的就是这个值）
    assert!(text.contains("claude-sonnet-4-20250514"), "实际：{text}");
    assert!(!text.contains("glm-4.6"), "真实模型名不该漏给客户端：{text}");

    // 用量从展开前的 JSON 里提出来了
    let entry = entry_value(&state, 0);
    assert_eq!(entry["tokens"]["input"], json!(6));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn a_passthrough_upstream_that_returns_json_is_also_expanded() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        // 没走转换（上游格式就是 Anthropic），但上游同样忽略了 stream 参数
        json_response(
            200,
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "glm-4.6",
                "content": [{ "type": "text", "text": "hello" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 5, "output_tokens": 1 }
            }),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "anthropic"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream")
        .json(&claude_body("claude-sonnet-4-20250514", true))
        .send()
        .await
        .unwrap();

    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    assert!(text.contains("event: message_start"), "实际：{text}");
    assert!(text.contains("message_stop"), "实际：{text}");

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn upstream_errors_are_rewritten_into_the_client_error_shape() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        json_response(429, json!({ "error": { "message": "rate limited", "type": "rate_limit_error" } }))
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&claude_body("claude-sonnet-4-20250514", false))
        .send()
        .await
        .unwrap();

    // 状态码保留，错误体换成 Anthropic 的形状：Claude Code 只认这一套
    assert_eq!(response.status(), 429);
    let payload: Value = response.json().await.unwrap();
    assert_eq!(payload["type"], json!("error"));
    assert_eq!(payload["error"]["type"], json!("rate_limit_error"));
    assert_eq!(payload["error"]["message"], json!("rate limited"));
    // 上游那份 OpenAI 形状的原文不该再出现
    assert!(payload.get("error").unwrap().get("code").is_none());

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn anthropic_errors_become_responses_errors_for_codex() {
    let captured = sink();
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), |_| {
        json_response(
            400,
            json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "bad request" } }),
        )
    })
    .await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "openai_responses", "gpt-5-codex", "anthropic"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/responses"))
        .header("content-type", "application/json")
        .json(&json!({
            "model": "gpt-5-codex",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);
    let payload: Value = response.json().await.unwrap();
    // Codex 认的是 {"error":{...}} 这个形状
    assert_eq!(payload["error"]["message"], json!("bad request"));
    assert_eq!(payload["error"]["type"], json!("invalid_request_error"));

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}

#[tokio::test]
async fn an_sse_body_is_passed_through_when_the_client_wanted_a_whole_response() {
    let captured = sink();
    let sse = concat!(
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (mock_port, mock_stop) = mock_upstream(captured.clone(), move |_| sse_response(sse.to_string())).await;

    let dir = TempDir::new("ccproxy-convert");
    let state = test_state(
        config_with_port(config_for(mock_port, "anthropic", "claude-sonnet-4-*", "openai_chat"), 0),
        &dir,
    );
    let port = start_proxy(&state).await;

    // 客户端要整块（stream:false、不带 accept），上游却回了 SSE：这一对转不了，
    // 但要原样透传而不是编一个 502 出来
    let response = client()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&claude_body("claude-sonnet-4-20250514", false))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    assert!(text.contains("chat.completion.chunk"), "实际：{text}");

    state.proxy.stop().await;
    let _ = mock_stop.send(());
}
