// 协议识别器单测：只看路径的判定，含前缀边界与「头/体特征不再影响结论」

use ccproxy_lib::proxy::detector::{detect_protocol, unknown_path_message};

#[test]
fn anthropic_paths() {
    assert_eq!(detect_protocol("/v1/messages"), Some("anthropic"));
    // 查询串不参与判定
    assert_eq!(detect_protocol("/v1/messages?beta=true"), Some("anthropic"));
    // 子路径（count_tokens 一类）跟着前缀一起进来
    assert_eq!(detect_protocol("/v1/messages/count_tokens"), Some("anthropic"));
}

#[test]
fn openai_paths() {
    assert_eq!(detect_protocol("/v1/chat/completions"), Some("openai"));
    assert_eq!(detect_protocol("/v1/responses"), Some("openai"));
    assert_eq!(detect_protocol("/v1/completions"), Some("openai"));
    assert_eq!(detect_protocol("/v1/embeddings"), Some("openai"));
    assert_eq!(detect_protocol("/v1/models"), Some("openai"));
    assert_eq!(detect_protocol("/v1/models/claude-sonnet-4-20250514"), Some("openai"));
    assert_eq!(detect_protocol("/v1/responses?stream=true"), Some("openai"));
}

#[test]
fn unknown_paths_are_none() {
    assert_eq!(detect_protocol("/"), None);
    assert_eq!(detect_protocol("/foo"), None);
    assert_eq!(detect_protocol("/v1/files"), None);
    // 前缀要按整段对齐，长得像不算命中
    assert_eq!(detect_protocol("/v1/messagesx"), None);
    assert_eq!(detect_protocol("/v1/modelsx"), None);
    assert_eq!(detect_protocol("/anthropic/v1/messages"), None);
}

#[test]
fn unknown_path_message_lists_known_paths() {
    let message = unknown_path_message("/foo");
    assert!(message.contains("/foo"));
    assert!(message.contains("/v1/messages"));
    assert!(message.contains("/v1/chat/completions"));
    assert!(message.contains("/v1/responses"));
}
