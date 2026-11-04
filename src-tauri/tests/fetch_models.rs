// 上游模型拉取单测：认证注入、成功解析、去重、错误场景

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, Response};
use axum::routing::any;
use axum::Router;
use serde_json::{json, Value};

use ccproxy_lib::proxy::fetch_models::fetch_upstream_models;

type Sink = Arc<Mutex<Vec<(String, HeaderMap)>>>;

async fn start_mock(app: Router) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
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

async fn mock(sink: Sink, status: u16, body: Value) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let app = Router::new().fallback(any(move |request: axum::extract::Request| {
        let sink = sink.clone();
        let payload = body.clone();
        async move {
            let url = request
                .uri()
                .path_and_query()
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            if let Ok(mut entries) = sink.lock() {
                entries.push((url, request.headers().clone()));
            }
            Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap()
        }
    }));
    start_mock(app).await
}

fn sink() -> Sink {
    Arc::new(Mutex::new(Vec::new()))
}

#[tokio::test]
async fn fetch_models_parses_dedupes_and_injects_anthropic_auth() {
    let captured = sink();
    let (port, stop) = mock(
        captured.clone(),
        200,
        json!({ "data": [{ "id": "glm-4.6" }, { "id": " glm-4.5 " }, { "id": "glm-4.6" }, { "object": "model" }] }),
    )
    .await;

    let client = reqwest::Client::new();
    let result = fetch_upstream_models(
        &client,
        &json!({ "apiFormat": "anthropic", "baseUrl": format!("http://127.0.0.1:{port}"), "token": "sk-real" }),
    )
    .await;

    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["models"], json!(["glm-4.6", "glm-4.5"]));

    let entries = captured.lock().unwrap();
    let (url, headers) = entries.last().unwrap();
    assert_eq!(url, "/v1/models?limit=1000");
    assert_eq!(headers["x-api-key"], "sk-real");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    drop(entries);
    let _ = stop.send(());
}

#[tokio::test]
async fn fetch_models_openai_chat_uses_bearer_and_standard_catalog_only() {
    let captured = sink();
    let (port, stop) = mock(
        captured.clone(),
        200,
        json!({ "data": [{ "id": "gpt-4o" }] }),
    )
    .await;

    let client = reqwest::Client::new();
    let result = fetch_upstream_models(
        &client,
        &json!({ "apiFormat": "openai_chat", "baseUrl": format!("http://127.0.0.1:{port}/v1"), "token": "sk-real" }),
    )
    .await;
    assert_eq!(result["models"], json!(["gpt-4o"]));

    let entries = captured.lock().unwrap();
    let urls: Vec<&str> = entries.iter().map(|(url, _)| url.as_str()).collect();
    assert_eq!(urls, ["/v1/models"]);
    // baseUrl 已带 /v1，请求路径里的 /v1 要去重
    let (_, headers) = &entries[0];
    assert_eq!(headers["authorization"], "Bearer sk-real");
    assert!(headers.get("x-api-key").is_none());
    drop(entries);
    let _ = stop.send(());
}

#[tokio::test]
async fn fetch_models_openai_responses_uses_codex_catalog_only() {
    let captured = sink();
    let captured_for_mock = captured.clone();
    let app = Router::new().fallback(any(move |request: axum::extract::Request| {
        let sink = captured_for_mock.clone();
        async move {
            let url = request
                .uri()
                .path_and_query()
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            if let Ok(mut entries) = sink.lock() {
                entries.push((url.clone(), request.headers().clone()));
            }
            let payload = json!({ "models": [{ "slug": "codex-only" }, { "slug": "codex-only" }] });
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap()
        }
    }));
    let (port, stop) = start_mock(app).await;

    let client = reqwest::Client::new();
    let result = fetch_upstream_models(
        &client,
        &json!({ "apiFormat": "openai_responses", "baseUrl": format!("http://127.0.0.1:{port}"), "token": "sk-real" }),
    )
    .await;

    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["models"], json!(["codex-only"]));

    let entries = captured.lock().unwrap();
    let urls: Vec<&str> = entries.iter().map(|(url, _)| url.as_str()).collect();
    assert_eq!(
        urls,
        [format!("/v1/models?client_version={}", env!("CARGO_PKG_VERSION"))]
    );
    drop(entries);
    let _ = stop.send(());
}

#[tokio::test]
async fn fetch_models_reports_status_and_shape_errors() {
    let client = reqwest::Client::new();

    let captured = sink();
    let (port, stop) = mock(captured.clone(), 500, json!({ "error": "boom" })).await;
    let result = fetch_upstream_models(
        &client,
        &json!({ "apiFormat": "anthropic", "baseUrl": format!("http://127.0.0.1:{port}"), "token": "t" }),
    )
    .await;
    assert_eq!(result["ok"], json!(false));
    assert!(result["error"].as_str().unwrap().contains("HTTP 500"));
    let _ = stop.send(());

    let captured = sink();
    let (port, stop) = mock(captured.clone(), 200, json!({ "models": [] })).await;
    let result = fetch_upstream_models(
        &client,
        &json!({ "apiFormat": "anthropic", "baseUrl": format!("http://127.0.0.1:{port}"), "token": "t" }),
    )
    .await;
    assert_eq!(result["ok"], json!(false));
    assert!(result["error"]
        .as_str()
        .unwrap()
        .contains("没有可识别的模型列表"));
    let _ = stop.send(());
}

#[tokio::test]
async fn fetch_models_reports_unreachable_upstream_without_throwing() {
    let client = reqwest::Client::new();

    // 明确格式连不上时报错，不抛异常；auto 与认不出的格式在发起请求前就拒绝。
    for api_format in ["anthropic", "openai_chat", "openai_responses"] {
        let result = fetch_upstream_models(
            &client,
            &json!({ "apiFormat": api_format, "baseUrl": "http://127.0.0.1:1", "token": "t" }),
        )
        .await;
        assert_eq!(result["ok"], json!(false), "{api_format} 应该报失败");
        let error = result["error"].as_str().unwrap();
        assert!(!error.trim().is_empty(), "{api_format} 应该返回错误原因");
    }
    for api_format in ["openai", "auto", "gemini"] {
        let result = fetch_upstream_models(
            &client,
            &json!({ "apiFormat": api_format, "baseUrl": "http://127.0.0.1:1", "token": "t" }),
        )
        .await;
        assert_eq!(result["ok"], json!(false), "{api_format} 应该报失败");
        assert!(
            result["error"].as_str().unwrap().contains("上游格式无效"),
            "{api_format}"
        );
    }
}
