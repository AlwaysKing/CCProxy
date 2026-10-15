//! Protocol conversion modules migrated from the CC Switch desktop app.
//!
//! 只保留 anthropic / openai_chat / openai_responses 三种线格式之间的转换；
//! 上游是 Gemini 的那几个模块（gemini_schema / gemini_shadow / streaming_gemini /
//! transform_gemini）没有搬过来。

pub(crate) mod codex_chat_common;
pub(crate) mod codex_chat_history;
pub(crate) mod codex_compaction;
pub(crate) mod codex_responses_sse;
pub(crate) mod inline_think;
pub(crate) mod reasoning_bridge;
pub(crate) mod streaming;
pub(crate) mod streaming_codex_anthropic;
pub(crate) mod streaming_codex_chat;
pub(crate) mod streaming_responses;
pub(crate) mod transform;
pub(crate) mod transform_codex_anthropic;
pub(crate) mod transform_codex_chat;
pub(crate) mod transform_codex_responses_namespace;
pub(crate) mod transform_codex_responses_xai_sanitize;
pub(crate) mod transform_responses;
