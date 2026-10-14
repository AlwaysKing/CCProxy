// 协议识别器单测：打分制在各种特征组合下的判定

use axum::http::HeaderMap;
use serde_json::json;

use ccproxy_lib::proxy::detector::detect_protocol;

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

fn anthropic_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 1024,
        "system": [{ "type": "text", "text": "x" }],
        "messages": []
    }))
    .unwrap()
}

fn openai_body() -> Vec<u8> {
    serde_json::to_vec(&json!({ "model": "gpt-4o", "messages": [], "temperature": 1 })).unwrap()
}

#[test]
fn path_v1_messages_is_anthropic() {
    let result = detect_protocol("/v1/messages", &headers(&[]), &anthropic_body());
    assert_eq!(result.protocol.as_deref(), Some("anthropic"));
    assert!(result.scores.anthropic > result.scores.openai);
}

#[test]
fn path_with_query_string_is_anthropic() {
    let result = detect_protocol("/v1/messages?beta=true", &headers(&[]), &anthropic_body());
    assert_eq!(result.protocol.as_deref(), Some("anthropic"));
}

#[test]
fn path_chat_completions_is_openai() {
    let result = detect_protocol("/v1/chat/completions", &headers(&[]), &openai_body());
    assert_eq!(result.protocol.as_deref(), Some("openai"));
}

#[test]
fn path_responses_is_openai() {
    let result = detect_protocol("/v1/responses", &headers(&[]), &openai_body());
    assert_eq!(result.protocol.as_deref(), Some("openai"));
}

#[test]
fn get_v1_models_without_body_is_openai() {
    let result = detect_protocol("/v1/models", &headers(&[]), &[]);
    assert_eq!(result.protocol.as_deref(), Some("openai"));
}

#[test]
fn anthropic_version_header_alone_is_anthropic() {
    let result = detect_protocol("/foo", &headers(&[("anthropic-version", "2023-06-01")]), &[]);
    assert_eq!(result.protocol.as_deref(), Some("anthropic"));
}

#[test]
fn body_features_alone_are_anthropic() {
    let result = detect_protocol("/", &headers(&[]), &anthropic_body());
    assert_eq!(result.protocol.as_deref(), Some("anthropic"));
}

#[test]
fn no_features_returns_null_with_scores_and_reasons() {
    let result = detect_protocol("/foo", &headers(&[]), b"{}");
    assert_eq!(result.protocol, None);
    assert_eq!(result.scores.anthropic, 0);
    assert_eq!(result.scores.openai, 0);
    assert!(result.reasons.is_empty() || result.reasons.iter().all(|item| !item.is_empty()));
}

#[test]
fn broken_json_body_does_not_panic() {
    let result = detect_protocol("/v1/messages", &headers(&[]), b"not json");
    assert_eq!(result.protocol.as_deref(), Some("anthropic"));
}

