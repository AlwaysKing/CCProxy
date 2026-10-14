// 上游转发：拼 URL、出站头清洗、首字节超时。
// 默认响应体原样透传；传入回写器时启用响应回写（见 server.rs）。

use std::time::Duration;

use axum::http::{HeaderMap, Method};
use bytes::Bytes;
use reqwest::Response;

// hop-by-hop 头与每请求必须重算的头，不透传
const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "proxy-authenticate",
    "proxy-authorization",
];
const REQUEST_HEADER_BLOCKLIST: [&str; 11] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "proxy-authenticate",
    "proxy-authorization",
    "host",
    "content-length",
    "accept-encoding",
];

// 超时三档：流式首字节 / 流式空闲 / 非流式整体
pub const FIRST_BYTE_TIMEOUT_MS: u64 = 60 * 1000;
pub const STREAM_IDLE_TIMEOUT_MS: u64 = 120 * 1000;
pub const NON_STREAM_TIMEOUT_MS: u64 = 600 * 1000;

// 去掉重复的版本段：baseUrl 已以 v1/v1beta 结尾而剩余路径又带同一段时只保留一份
fn drop_duplicated_version_segment(base_url: &str, remaining_path: &str) -> String {
    let base_trimmed = base_url.trim_end_matches('/');
    let base_segments: Vec<&str> = base_trimmed.split('/').filter(|segment| !segment.is_empty()).collect();
    let base_tail = base_segments.last().copied().unwrap_or("");
    let mut segments: Vec<&str> = remaining_path.split('/').filter(|segment| !segment.is_empty()).collect();
    if !base_tail.is_empty()
        && segments.first().copied() == Some(base_tail)
        && is_version_segment(base_tail)
    {
        segments.remove(0);
    }
    if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn is_version_segment(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('v') else {
        return false;
    };
    let digits: String = rest.chars().take_while(|character| character.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    let tail = &rest[digits.len()..];
    if tail.is_empty() {
        return true;
    }
    match tail.strip_prefix("beta") {
        // v1beta 与 v1beta2 都算版本段
        Some(beta) => beta.chars().all(|character| character.is_ascii_digit()),
        None => false,
    }
}

pub fn build_upstream_url(base_url: &str, remaining_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}{}", drop_duplicated_version_segment(base, remaining_path))
}

// 出站头清洗：剥 blocklist（HeaderMap 的键本身已是小写）
pub fn sanitize_outbound_headers(headers: &HeaderMap) -> HeaderMap {
    let mut outbound = HeaderMap::new();
    for (name, value) in headers.iter() {
        if REQUEST_HEADER_BLOCKLIST.contains(&name.as_str()) {
            continue;
        }
        outbound.append(name.clone(), value.clone());
    }
    outbound
}

// 上游响应头透传给客户端：剥掉 hop-by-hop（框架自己管分帧），其余原样保留
pub fn passthrough_response_headers(headers: &HeaderMap, drop_length: bool) -> HeaderMap {
    let mut outbound = HeaderMap::new();
    for (name, value) in headers.iter() {
        if HOP_BY_HOP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if drop_length && name.as_str() == "content-length" {
            continue;
        }
        outbound.append(name.clone(), value.clone());
    }
    outbound
}

// 发请求，带上首字节超时。失败时返回给用户看的错误文案
pub async fn send_upstream(
    client: &reqwest::Client,
    url: &str,
    method: &Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, String> {
    let request = client
        .request(method.clone(), url)
        .headers(headers)
        .body(body);
    match tokio::time::timeout(Duration::from_millis(FIRST_BYTE_TIMEOUT_MS), request.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(describe_error(&error)),
        Err(_) => Err(format!("上游 {FIRST_BYTE_TIMEOUT_MS}ms 内无响应")),
    }
}

fn describe_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "上游请求超时".to_string();
    }
    if error.is_connect() {
        return format!("连接上游失败：{error}");
    }
    error.to_string()
}

