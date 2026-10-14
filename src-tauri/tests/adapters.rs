// 协议适配器单测：入站凭据剥离、按协议注入认证、model 改写、响应 model 回写、用量提取

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::{json, Value};

use ccproxy_lib::proxy::adapters::{get_adapter, strip_inbound_credentials, ResponseRewriter};

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
    }
    map
}

#[test]
fn strips_inbound_credentials_only() {
    let mut map = headers(&[
        ("content-type", "application/json"),
        ("authorization", "Bearer fake"),
        ("x-api-key", "fake"),
        ("api-key", "fake"),
        ("x-goog-api-key", "fake"),
        ("proxy-authorization", "Basic fake"),
        ("anthropic-version", "2023-06-01"),
        ("user-agent", "test"),
    ]);
    strip_inbound_credentials(&mut map);
    assert_eq!(map.get("content-type").unwrap(), "application/json");
    assert_eq!(map.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(map.get("user-agent").unwrap(), "test");
    assert!(map.get("authorization").is_none());
    assert!(map.get("x-api-key").is_none());
    assert!(map.get("api-key").is_none());
    assert!(map.get("x-goog-api-key").is_none());
    assert!(map.get("proxy-authorization").is_none());
}

#[test]
fn anthropic_adapter_injects_x_api_key() {
    let adapter = get_adapter("anthropic").unwrap();
    let mut map = HeaderMap::new();
    adapter.apply_auth(&mut map, "sk-real");
    assert_eq!(map.get("x-api-key").unwrap(), "sk-real");
    assert!(map.get("authorization").is_none());
}

#[test]
fn openai_adapter_injects_bearer() {
    let adapter = get_adapter("openai").unwrap();
    let mut map = HeaderMap::new();
    adapter.apply_auth(&mut map, "sk-real");
    assert_eq!(map.get("authorization").unwrap(), "Bearer sk-real");
    assert!(map.get("x-api-key").is_none());
}

#[test]
fn both_adapters_rewrite_top_level_model() {
    for id in ["anthropic", "openai"] {
        let adapter = get_adapter(id).unwrap();
        let mut payload = json!({ "model": "claude-sonnet-4", "messages": [{ "role": "user", "content": "hi" }] });
        adapter.rewrite_model(&mut payload, "glm-4.6");
        assert_eq!(payload["model"], json!("glm-4.6"));
        assert_eq!(payload["messages"][0]["content"], json!("hi"));
    }
}

#[test]
fn unknown_protocol_errors() {
    let error = get_adapter("gemini").unwrap_err();
    assert!(error.contains("未知协议"));
}

#[test]
fn rewrites_model_in_non_stream_body() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let body = Bytes::from(serde_json::to_vec(&json!({ "id": "msg_1", "model": "glm-5.3", "content": [] })).unwrap());
    let out = rewriter.rewrite_body(&body);
    let payload: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["model"], json!("cnsinda-glm-5.3"));
    assert_eq!(payload["id"], json!("msg_1"));
}

#[test]
fn non_json_and_modelless_bodies_pass_through() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let not_json = Bytes::from_static(b"<html>upstream error page</html>");
    assert_eq!(rewriter.rewrite_body(&not_json), not_json);
    let no_model = Bytes::from(serde_json::to_vec(&json!({ "error": { "type": "not_found_error" } })).unwrap());
    assert_eq!(rewriter.rewrite_body(&no_model), no_model);
}

#[test]
fn anthropic_stream_rewrites_only_message_start_model() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let event = Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5.3\",\"role\":\"assistant\"}}\n\n",
    );
    let out = String::from_utf8(rewriter.rewrite_sse_event(&event).to_vec()).unwrap();
    assert!(out.starts_with("event: message_start\n"));
    assert!(out.ends_with("\n\n"));
    let payload: Value = serde_json::from_str(out.split('\n').nth(1).unwrap()[5..].trim()).unwrap();
    assert_eq!(payload["message"]["model"], json!("cnsinda-glm-5.3"));

    // 非 message_start 事件不带 model，改写后内容不变
    let delta = Bytes::from_static(
        b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
    );
    assert_eq!(rewriter.rewrite_sse_event(&delta), delta);
}

#[test]
fn openai_stream_rewrites_every_data_line() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "gpt-4o-proxy");
    let event = Bytes::from_static(b"data: {\"id\":\"x\",\"model\":\"deepseek-chat\",\"choices\":[]}\n\ndata: [DONE]\n\n");
    let out = String::from_utf8(rewriter.rewrite_sse_event(&event).to_vec()).unwrap();
    let lines: Vec<&str> = out.split('\n').collect();
    let payload: Value = serde_json::from_str(lines[0][5..].trim()).unwrap();
    assert_eq!(payload["model"], json!("gpt-4o-proxy"));
    assert_eq!(lines[2], "data: [DONE]");
}

#[test]
fn multibyte_text_survives_event_rewrite() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let text = "跑通";
    let event = Bytes::from(format!(
        "event: content_block_delta\ndata: {}\n\n",
        serde_json::to_string(&json!({ "type": "content_block_delta", "delta": { "text": text } })).unwrap()
    ));
    let out = String::from_utf8(rewriter.rewrite_sse_event(&event).to_vec()).unwrap();
    let payload: Value = serde_json::from_str(out.split('\n').nth(1).unwrap()[5..].trim()).unwrap();
    assert_eq!(payload["delta"]["text"], json!("跑通"));
}

#[test]
fn anthropic_usage_from_top_level_usage() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let body = Bytes::from(
        serde_json::to_vec(&json!({ "model": "glm-5.3", "usage": { "input_tokens": 12, "output_tokens": 34 } })).unwrap(),
    );
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(12), Some(34)));
}

#[test]
fn anthropic_stream_usage_is_overwritten_not_summed() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    rewriter.rewrite_sse_event(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5.3\",\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n",
    );
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(100), Some(1)));
    rewriter.rewrite_sse_event(
        b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":257}}\n\n",
    );
    // message_delta 的 output_tokens 是最终累计值，再累加就翻倍了
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(100), Some(257)));
}

#[test]
fn openai_usage_from_prompt_and_completion_tokens() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "gpt-4o-proxy");
    let body = Bytes::from(
        serde_json::to_vec(&json!({ "model": "glm", "usage": { "prompt_tokens": 9, "completion_tokens": 8 } })).unwrap(),
    );
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(9), Some(8)));
}

#[test]
fn openai_stream_usage_only_when_last_frame_reports() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "gpt-4o-proxy");
    rewriter.rewrite_sse_event(b"data: {\"id\":\"x\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n");
    assert!(rewriter.usage().is_none());
    rewriter.rewrite_sse_event(
        b"data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":5}}\n\ndata: [DONE]\n\n",
    );
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(7), Some(5)));
}

#[test]
fn unreported_usage_is_none_while_zero_is_zero() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    assert!(rewriter.usage().is_none());
    let body = Bytes::from(serde_json::to_vec(&json!({ "usage": { "input_tokens": 0, "output_tokens": 0 } })).unwrap());
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(0), Some(0)));
}

