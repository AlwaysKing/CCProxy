// 上游转发单测：URL 拼接（含重复版本段去重）、出站头清洗

use axum::http::HeaderMap;

use ccproxy_lib::proxy::upstream::{build_upstream_url, sanitize_outbound_headers};

#[test]
fn base_url_without_version_segment() {
    assert_eq!(
        build_upstream_url("https://api.anthropic.com", "/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn duplicated_version_segment_is_dropped() {
    assert_eq!(
        build_upstream_url("http://upstream.test:9005/v1", "/v1/responses"),
        "http://upstream.test:9005/v1/responses"
    );
}

#[test]
fn base_url_with_v1_and_path_without_it() {
    assert_eq!(
        build_upstream_url("http://upstream.test:9005/v1", "/chat/completions"),
        "http://upstream.test:9005/v1/chat/completions"
    );
}

#[test]
fn trailing_slashes_are_trimmed() {
    assert_eq!(
        build_upstream_url("http://upstream.test/api/anthropic/", "/v1/messages"),
        "http://upstream.test/api/anthropic/v1/messages"
    );
}

#[test]
fn v1beta_tail_is_recognized_as_version_segment() {
    assert_eq!(
        build_upstream_url("https://generativelanguage.test/v1beta", "/v1beta/models"),
        "https://generativelanguage.test/v1beta/models"
    );
}

#[test]
fn sanitize_strips_hop_by_hop_and_recomputed_headers() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("host", "127.0.0.1:18888"),
        ("connection", "keep-alive"),
        ("transfer-encoding", "chunked"),
        ("content-length", "999"),
        ("accept-encoding", "gzip"),
        ("content-type", "application/json"),
        ("anthropic-version", "2023-06-01"),
    ] {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
    }
    let outbound = sanitize_outbound_headers(&headers);
    assert!(outbound.get("host").is_none());
    assert!(outbound.get("connection").is_none());
    assert!(outbound.get("transfer-encoding").is_none());
    assert!(outbound.get("content-length").is_none());
    assert!(outbound.get("accept-encoding").is_none());
    assert_eq!(outbound.get("content-type").unwrap(), "application/json");
    assert_eq!(outbound.get("anthropic-version").unwrap(), "2023-06-01");
}

