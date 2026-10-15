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

// responses 的流式事件把模型名放在 response.model 上（真实网关的 created / in_progress /
// 终止事件都是这个形状），只在顶层换名的话客户端会看到上游真实模型名
#[test]
fn openai_responses_stream_rewrites_nested_model() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "cn:deepseek-v4-flash");
    let event = Bytes::from_static(
        b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-v4.1-flash\",\"status\":\"in_progress\",\"usage\":null},\"sequence_number\":0}\n\n",
    );
    let out = String::from_utf8(rewriter.rewrite_sse_event(&event).to_vec()).unwrap();
    let payload: Value = serde_json::from_str(out.split('\n').nth(1).unwrap()[5..].trim()).unwrap();
    assert_eq!(payload["response"]["model"], json!("cn:deepseek-v4-flash"));
    assert_eq!(payload["type"], json!("response.created"));

    // 不带 model 的事件原样透传：不能被塞进一个凭空多出来的 model
    let delta = Bytes::from_static(
        b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\",\"sequence_number\":3}\n\n",
    );
    assert_eq!(rewriter.rewrite_sse_event(&delta), delta);

    // 非流式的 responses 响应体就是响应对象本身，model 在顶层，照旧换得动
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "object": "response", "id": "resp_1", "status": "completed", "model": "deepseek-v4.1-flash", "output": []
        }))
        .unwrap(),
    );
    let rewritten: Value = serde_json::from_slice(&rewriter.rewrite_body(&body)).unwrap();
    assert_eq!(rewritten["model"], json!("cn:deepseek-v4-flash"));
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

// anthropic 的 input_tokens 只算没走缓存的那部分，缓存读与写是另外两项；
// 对外口径的 input 是「全部输入」，所以要把三项加起来（缓存写不单独列出来）
#[test]
fn anthropic_usage_folds_cache_read_and_write_into_input() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "glm-5.3",
            "usage": {
                "input_tokens": 12,
                "output_tokens": 34,
                "cache_read_input_tokens": 1000,
                "cache_creation_input_tokens": 200
            }
        }))
        .unwrap(),
    );
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(1212), Some(34)));
    assert_eq!(usage.cache_read, Some(1000));
}

// message_delta 只报 output 时，message_start 里的输入与缓存不能被抹掉
#[test]
fn anthropic_stream_keeps_input_and_cache_when_delta_only_reports_output() {
    let rewriter = ResponseRewriter::new(get_adapter("anthropic").unwrap(), "cnsinda-glm-5.3");
    rewriter.rewrite_sse_event(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5.3\",\"usage\":{\"input_tokens\":4,\"cache_read_input_tokens\":20000,\"cache_creation_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    );
    rewriter.rewrite_sse_event(
        b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":150}}\n\n",
    );
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(20004), Some(150)));
    assert_eq!(usage.cache_read, Some(20000));
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

// chat completions 的缓存命中在 prompt_tokens_details.cached_tokens 里，
// 而 prompt_tokens 已经含了它——缓存只做明细，不能加进 input
#[test]
fn openai_chat_usage_reads_cached_tokens_from_details() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "gpt-4o-proxy");
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "glm",
            "usage": {
                "prompt_tokens": 900,
                "completion_tokens": 8,
                "prompt_tokens_details": { "cached_tokens": 640 }
            }
        }))
        .unwrap(),
    );
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(900), Some(8)));
    assert_eq!(usage.cache_read, Some(640));
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

// Codex（wire_api = "responses"）走的是 /v1/responses：非流式用量在响应顶层，字段是
// input_tokens / output_tokens，不是 chat completions 的 prompt_tokens / completion_tokens
#[test]
fn openai_responses_non_stream_usage_from_input_output_tokens() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "deepseek-flash");
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "object": "response",
            "model": "deepseek-flash",
            "status": "completed",
            "usage": {
                "input_tokens": 16613,
                "input_tokens_details": { "cached_tokens": 12928 },
                "output_tokens": 289,
                "output_tokens_details": { "reasoning_tokens": 93 },
                "total_tokens": 16902
            }
        }))
        .unwrap(),
    );
    rewriter.rewrite_body(&body);
    let usage = rewriter.usage().expect("responses 非流式响应应能取到用量");
    assert_eq!((usage.input, usage.output), (Some(16613), Some(289)));
    assert_eq!(usage.cache_read, Some(12928));
}

// 流式 responses 的最后一条事件是 response.completed，usage 挂在 response 对象里
#[test]
fn openai_responses_stream_usage_from_completed_event() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "deepseek-flash");
    rewriter.rewrite_sse_event(
        b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
    );
    assert!(rewriter.usage().is_none(), "中间事件不带用量时不该凭空有数");
    rewriter.rewrite_sse_event(
        b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-flash\",\"status\":\"completed\",\"usage\":{\"input_tokens\":120,\"input_tokens_details\":{\"cached_tokens\":64},\"output_tokens\":45,\"output_tokens_details\":{\"reasoning_tokens\":12},\"total_tokens\":165}}}\n\n",
    );
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(120), Some(45)));
    // input_tokens 已含缓存命中，缓存读只作明细
    assert_eq!(usage.cache_read, Some(64));
}

// 实测（deepseek 网关）两个真实形状：被 max_output_tokens 截断时收尾事件是
// response.incomplete，用量照样在 response.usage 上；前面那些事件给的是 "usage": null，
// 那不是「0」而是「还没到」，不能因此记一笔 0
#[test]
fn openai_responses_usage_reads_incomplete_event_and_ignores_null_usage() {
    let rewriter = ResponseRewriter::new(get_adapter("openai").unwrap(), "deepseek-flash");
    rewriter.rewrite_sse_event(
        b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-flash\",\"status\":\"in_progress\",\"usage\":null}}\n\n",
    );
    assert!(rewriter.usage().is_none(), "usage 为 null 时应算未上报");
    rewriter.rewrite_sse_event(
        b"event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"model\":\"deepseek-flash\",\"status\":\"incomplete\",\"usage\":{\"input_tokens\":31,\"input_tokens_details\":{\"cached_tokens\":0},\"output_tokens\":24,\"output_tokens_details\":{\"reasoning_tokens\":24},\"total_tokens\":55}}}\n\n",
    );
    let usage = rewriter.usage().unwrap();
    assert_eq!((usage.input, usage.output), (Some(31), Some(24)));
    assert_eq!(usage.cache_read, Some(0));
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

