use crate::convert::sse::{append_utf8_safe, parse_sse_block, sse_bytes, take_sse_block};
use crate::convert::usage_shape;
use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

type UpstreamError = Box<dyn std::error::Error + Send + Sync>;
type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, UpstreamError>> + Send>>;

pub(crate) struct ConvertingStream {
    input: UpstreamStream,
    buffer: String,
    remainder: Vec<u8>,
    eof: bool,
    state: Converter,
    pending: Vec<Bytes>,
    done: bool,
}

enum Converter {
    ChatToAnthropic(ChatToAnthropicState),
    ResponsesToAnthropic(ResponsesToAnthropicState),
    ChatToResponses(ChatToResponsesState),
    AnthropicToResponses(AnthropicToResponsesState),
}

impl ConvertingStream {
    pub(crate) fn new(
        client: &str,
        upstream: &str,
        input: UpstreamStream,
    ) -> Result<Self, String> {
        let state = match (client, upstream) {
            ("anthropic", "openai_chat") => Converter::ChatToAnthropic(Default::default()),
            ("anthropic", "openai_responses") => {
                Converter::ResponsesToAnthropic(Default::default())
            }
            ("openai_responses", "openai_chat") => {
                Converter::ChatToResponses(Default::default())
            }
            ("openai_responses", "anthropic") => {
                Converter::AnthropicToResponses(Default::default())
            }
            _ => {
                return Err(format!(
                    "unsupported streaming conversion: client={client}, upstream={upstream}"
                ));
            }
        };

        Ok(Self {
            input,
            buffer: String::new(),
            remainder: Vec::new(),
            eof: false,
            state,
            pending: Vec::new(),
            done: false,
        })
    }

    fn process_block(&mut self, block: &str) -> Vec<Bytes> {
        let Some(event) = parse_sse_block(block) else {
            return Vec::new();
        };

        if event.data.trim() == "[DONE]" {
            return self.state.on_done();
        }

        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            return Vec::new();
        };
        let event_name = event
            .event
            .or_else(|| value.get("type").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        self.state.handle_event(&event_name, &value)
    }
}

impl Stream for ConvertingStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(bytes) = this.pending.first().cloned() {
                this.pending.remove(0);
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.done {
                return Poll::Ready(None);
            }

            if let Some(block) = take_sse_block(&mut this.buffer) {
                let events = this.process_block(&block);
                this.pending = events;
                if this.state.is_terminal() && this.pending.is_empty() {
                    this.done = true;
                }
                continue;
            }

            if !this.eof {
                match this.input.as_mut().poll_next(cx) {
                    Poll::Ready(Some(Ok(bytes))) => {
                        append_utf8_safe(&mut this.buffer, &mut this.remainder, &bytes);
                        continue;
                    }
                    Poll::Ready(Some(Err(_))) => {
                        let events = this.state.fail_stream_error();
                        this.pending = events;
                        this.eof = true;
                        this.done = true;
                        continue;
                    }
                    Poll::Ready(None) => {
                        this.eof = true;
                        if !this.buffer.trim().is_empty() {
                            this.buffer.push_str("\n\n");
                        }
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            let events = this.state.finish();
            this.pending = events;
            this.done = true;
            continue;
        }
    }
}

impl Converter {
    fn handle_event(&mut self, event_name: &str, value: &Value) -> Vec<Bytes> {
        match self {
            Converter::ChatToAnthropic(state) => state.handle_event(event_name, value),
            Converter::ResponsesToAnthropic(state) => state.handle_event(event_name, value),
            Converter::ChatToResponses(state) => state.handle_event(event_name, value),
            Converter::AnthropicToResponses(state) => state.handle_event(event_name, value),
        }
    }

    fn on_done(&mut self) -> Vec<Bytes> {
        match self {
            Converter::ChatToAnthropic(state) => state.finish(),
            Converter::ResponsesToAnthropic(state) => state.finish(),
            Converter::ChatToResponses(state) => state.finish(),
            Converter::AnthropicToResponses(state) => state.finish(),
        }
    }

    fn finish(&mut self) -> Vec<Bytes> {
        match self {
            Converter::ChatToAnthropic(state) => state.finish(),
            Converter::ResponsesToAnthropic(state) => state.finish(),
            Converter::ChatToResponses(state) => state.finish(),
            Converter::AnthropicToResponses(state) => state.finish(),
        }
    }

    fn fail_stream_error(&mut self) -> Vec<Bytes> {
        match self {
            Converter::ChatToAnthropic(state) => {
                state.fail("Stream error", "stream_error")
            }
            Converter::ResponsesToAnthropic(state) => {
                state.fail("Stream error", "stream_error")
            }
            Converter::ChatToResponses(state) => {
                state.fail("Stream error", "stream_error")
            }
            Converter::AnthropicToResponses(state) => {
                state.fail("Stream error", "stream_error")
            }
        }
    }

    fn is_terminal(&self) -> bool {
        match self {
            Converter::ChatToAnthropic(state) => state.is_terminal(),
            Converter::ResponsesToAnthropic(state) => state.is_terminal(),
            Converter::ChatToResponses(state) => state.is_terminal(),
            Converter::AnthropicToResponses(state) => state.is_terminal(),
        }
    }
}

fn anthropic_error_sse(message: &str, error_type: &str) -> Bytes {
    sse_bytes(
        "error",
        &json!({
            "type": "error",
            "error": { "type": error_type, "message": message }
        }),
    )
}

fn responses_failed_sse(message: &str, error_type: &str) -> Bytes {
    sse_bytes(
        "response.failed",
        &json!({
            "type": "response.failed",
            "response": {
                "object": "response",
                "status": "failed",
                "error": { "message": message, "type": error_type }
            }
        }),
    )
}

fn chat_delta_reasoning(delta: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = delta.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    if let Some(reasoning) = delta.get("reasoning") {
        for key in ["content", "text", "summary"] {
            if let Some(text) = reasoning.get(key).and_then(Value::as_str) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn canonical_tool_arguments(arguments: &str) -> String {
    if arguments.trim().is_empty() {
        return "{}".to_string();
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => serde_json::to_string(&value).unwrap_or_else(|_| arguments.to_string()),
        Err(_) => arguments.to_string(),
    }
}

fn map_chat_finish_reason(reason: &str) -> &'static str {
    match reason {
        "tool_calls" | "function_call" => "tool_use",
        "stop" => "end_turn",
        "length" => "max_tokens",
        _ => "end_turn",
    }
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions SSE -> Anthropic Messages SSE
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum NonToolBlockKind {
    Text,
    Thinking,
}

#[derive(Default)]
struct ChatToolState {
    anthropic_index: u32,
    id: String,
    name: String,
    started: bool,
    pending_args: String,
    open: bool,
}

#[derive(Default)]
struct ChatToAnthropicState {
    message_id: String,
    model: String,
    next_content_index: u32,
    message_started: bool,
    current_kind: Option<NonToolBlockKind>,
    current_index: Option<u32>,
    next_tool_slot: usize,
    tools: BTreeMap<usize, ChatToolState>,
    open_tools: BTreeSet<u32>,
    latest_usage: Value,
    pending_stop_reason: Option<String>,
    message_delta_emitted: bool,
    message_stop_emitted: bool,
    has_output: bool,
    failed: bool,
}

impl ChatToAnthropicState {
    fn ensure_message_start(&mut self) -> Vec<Bytes> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        vec![sse_bytes(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                }
            }),
        )]
    }

    fn close_current_block(&mut self) -> Vec<Bytes> {
        let Some(index) = self.current_index.take() else {
            return Vec::new();
        };
        self.current_kind = None;
        vec![sse_bytes(
            "content_block_stop",
            &json!({ "type": "content_block_stop", "index": index }),
        )]
    }

    fn push_reasoning(&mut self, text: &str) -> Vec<Bytes> {
        let mut events = self.ensure_message_start();
        if self.current_kind != Some(NonToolBlockKind::Thinking) {
            events.extend(self.close_current_block());
            let index = self.next_content_index;
            self.next_content_index += 1;
            self.current_kind = Some(NonToolBlockKind::Thinking);
            self.current_index = Some(index);
            events.push(sse_bytes(
                "content_block_start",
                &json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": { "type": "thinking", "thinking": "" }
                }),
            ));
        }
        self.has_output = true;
        if let Some(index) = self.current_index {
            events.push(sse_bytes(
                "content_block_delta",
                &json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": { "type": "thinking_delta", "thinking": text }
                }),
            ));
        }
        events
    }

    fn push_text(&mut self, text: &str) -> Vec<Bytes> {
        let mut events = self.ensure_message_start();
        if self.current_kind != Some(NonToolBlockKind::Text) {
            events.extend(self.close_current_block());
            let index = self.next_content_index;
            self.next_content_index += 1;
            self.current_kind = Some(NonToolBlockKind::Text);
            self.current_index = Some(index);
            events.push(sse_bytes(
                "content_block_start",
                &json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": { "type": "text", "text": "" }
                }),
            ));
        }
        self.has_output = true;
        if let Some(index) = self.current_index {
            events.push(sse_bytes(
                "content_block_delta",
                &json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": { "type": "text_delta", "text": text }
                }),
            ));
        }
        events
    }

    fn push_tool_calls(&mut self, tool_calls: &[Value]) -> Vec<Bytes> {
        let mut events = self.ensure_message_start();
        events.extend(self.close_current_block());

        for tool_call in tool_calls {
            let chat_index = tool_call
                .get("index")
                .and_then(Value::as_u64)
                .map(|index| index as usize)
                .unwrap_or_else(|| {
                    let index = self.next_tool_slot;
                    self.next_tool_slot += 1;
                    index
                });
            let function = tool_call.get("function").cloned().unwrap_or_else(|| json!({}));
            let args = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            let state = self.tools.entry(chat_index).or_insert_with(|| ChatToolState {
                anthropic_index: {
                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    index
                },
                ..Default::default()
            });
            if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    state.id = id.to_string();
                }
            }
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                if !name.is_empty() {
                    state.name = name.to_string();
                }
            }

            let index = state.anthropic_index;
            if !state.started && !state.id.is_empty() && !state.name.is_empty() {
                state.started = true;
                state.open = true;
                self.open_tools.insert(index);
                events.push(sse_bytes(
                    "content_block_start",
                    &json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {
                            "type": "tool_use",
                            "id": state.id,
                            "name": state.name
                        }
                    }),
                ));
                if !state.pending_args.is_empty() {
                    let pending = std::mem::take(&mut state.pending_args);
                    events.push(sse_bytes(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "input_json_delta", "partial_json": pending }
                        }),
                    ));
                }
            } else if !args.is_empty() {
                if state.started {
                    events.push(sse_bytes(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        }),
                    ));
                } else {
                    state.pending_args.push_str(&args);
                }
            }
            self.has_output = true;
        }

        events
    }

    fn finalize_tool_blocks(&mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        let indices: Vec<u32> = self.open_tools.iter().copied().collect();
        for index in indices {
            events.push(sse_bytes(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            ));
            self.open_tools.remove(&index);
        }
        events
    }

    fn handle_event(&mut self, event_name: &str, value: &Value) -> Vec<Bytes> {
        if event_name == "error" || value.get("error").is_some() {
            let error = value.get("error").unwrap_or(value);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("OpenAI Chat stream returned an error");
            return self.fail(message, "upstream_error");
        }

        if event_name != "" && event_name != "chat.completion.chunk" {
            return Vec::new();
        }

        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Vec::new();
        };

        if let Some(id) = value.get("id").and_then(Value::as_str) {
            if self.message_id.is_empty() {
                self.message_id = id.to_string();
            }
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            if self.model.is_empty() {
                self.model = model.to_string();
            }
        }
        if let Some(usage) = value.get("usage").filter(|value| value.is_object()) {
            self.latest_usage = usage_shape::openai_usage_to_anthropic(Some(usage));
        }

        let mut events = Vec::new();
        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = chat_delta_reasoning(delta) {
                events.extend(self.push_reasoning(&reasoning));
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    events.extend(self.push_text(content));
                }
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                events.extend(self.push_tool_calls(tool_calls));
            }
        }
        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            if self.pending_stop_reason.is_none() {
                self.pending_stop_reason = Some(map_chat_finish_reason(finish_reason).to_string());
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<Bytes> {
        if self.message_stop_emitted || self.failed {
            return Vec::new();
        }

        let mut events = self.close_current_block();
        events.extend(self.finalize_tool_blocks());

        if !self.message_started && !self.has_output && self.pending_stop_reason.is_none() {
            events.push(anthropic_error_sse(
                "OpenAI Chat stream ended before sending finish_reason",
                "stream_truncated",
            ));
            self.failed = true;
            return events;
        }

        events.extend(self.ensure_message_start());
        if !self.message_delta_emitted {
            self.message_delta_emitted = true;
            let usage = if self.latest_usage.is_object() {
                self.latest_usage.clone()
            } else {
                json!({ "input_tokens": 0, "output_tokens": 0 })
            };
            let stop_reason = self
                .pending_stop_reason
                .clone()
                .unwrap_or_else(|| "end_turn".to_string());
            events.push(sse_bytes(
                "message_delta",
                &json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": stop_reason, "stop_sequence": Value::Null },
                    "usage": usage
                }),
            ));
        }
        events.push(sse_bytes("message_stop", &json!({ "type": "message_stop" })));
        self.message_stop_emitted = true;
        events
    }

    fn fail(&mut self, message: &str, error_type: &str) -> Vec<Bytes> {
        if self.failed {
            return Vec::new();
        }
        self.failed = true;
        vec![anthropic_error_sse(message, error_type)]
    }

    fn is_terminal(&self) -> bool {
        self.failed || self.message_stop_emitted
    }
}

// ---------------------------------------------------------------------------
// OpenAI Responses SSE -> Anthropic Messages SSE
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ResponsesToolState {
    id: String,
    name: String,
    args: String,
    had_delta: bool,
}

#[derive(Default)]
struct ResponsesReasoningState {
    text: String,
}

#[derive(Default)]
struct ResponsesToAnthropicState {
    message_id: String,
    model: String,
    message_started: bool,
    next_content_index: u32,
    current_text_index: Option<u32>,
    open_indices: BTreeSet<u32>,
    tool_by_item_id: HashMap<String, u32>,
    tool_by_index: BTreeMap<u32, ResponsesToolState>,
    reasoning_by_item_id: HashMap<String, u32>,
    reasoning_by_index: BTreeMap<u32, ResponsesReasoningState>,
    last_tool_index: Option<u32>,
    has_tool_use: bool,
    has_substantive_output: bool,
    message_delta_emitted: bool,
    terminated: bool,
    failed: bool,
}

impl ResponsesToAnthropicState {
    fn ensure_message_start(&mut self) -> Vec<Bytes> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        vec![sse_bytes(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                }
            }),
        )]
    }

    fn next_index(&mut self) -> u32 {
        let index = self.next_content_index;
        self.next_content_index += 1;
        index
    }

    fn close_text_block(&mut self) -> Vec<Bytes> {
        let Some(index) = self.current_text_index.take() else {
            return Vec::new();
        };
        if self.open_indices.remove(&index) {
            vec![sse_bytes(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            )]
        } else {
            Vec::new()
        }
    }

    fn start_text_block(&mut self) -> Vec<Bytes> {
        if let Some(index) = self.current_text_index {
            if self.open_indices.contains(&index) {
                return Vec::new();
            }
        }
        let index = self.next_index();
        self.current_text_index = Some(index);
        self.open_indices.insert(index);
        self.has_substantive_output = true;
        vec![sse_bytes(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" }
            }),
        )]
    }

    fn start_tool_block(&mut self, index: u32, id: &str, name: &str) -> Vec<Bytes> {
        if self.open_indices.contains(&index) {
            return Vec::new();
        }
        self.open_indices.insert(index);
        self.has_substantive_output = true;
        vec![sse_bytes(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "tool_use", "id": id, "name": name }
            }),
        )]
    }

    fn close_tool_block(&mut self, index: u32) -> Vec<Bytes> {
        if self.open_indices.remove(&index) {
            vec![sse_bytes(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            )]
        } else {
            Vec::new()
        }
    }

    fn start_reasoning_block(&mut self, index: u32) -> Vec<Bytes> {
        if self.open_indices.contains(&index) {
            return Vec::new();
        }
        self.open_indices.insert(index);
        self.has_substantive_output = true;
        vec![sse_bytes(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
        )]
    }

    fn close_reasoning_block(&mut self, index: u32) -> Vec<Bytes> {
        if self.open_indices.remove(&index) {
            vec![sse_bytes(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            )]
        } else {
            Vec::new()
        }
    }

    fn response_usage_to_anthropic(usage: &Value) -> Value {
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("prompt_tokens").and_then(Value::as_u64))
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("completion_tokens").and_then(Value::as_u64))
            .unwrap_or(0);
        let mut result = json!({ "input_tokens": input, "output_tokens": output });
        if let Some(cached) = usage
            .pointer("/input_tokens_details/cached_tokens")
            .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
        {
            result["cache_read_input_tokens"] = json!(cached);
        }
        if let Some(write) = usage
            .pointer("/input_tokens_details/cache_write_tokens")
            .or_else(|| usage.pointer("/prompt_tokens_details/cache_write_tokens"))
            .and_then(Value::as_u64)
        {
            result["cache_creation_input_tokens"] = json!(write);
        }
        result
    }

    fn terminal_events(&mut self, usage: Value, incomplete_reason: Option<&str>) -> Vec<Bytes> {
        let mut events = Vec::new();
        let indices: Vec<u32> = self.open_indices.iter().copied().collect();
        for index in indices {
            events.extend(self.close_text_block());
            events.extend(self.close_tool_block(index));
            events.extend(self.close_reasoning_block(index));
        }
        events.extend(self.ensure_message_start());

        let stop_reason = if incomplete_reason.is_some() {
            "max_tokens".to_string()
        } else if self.has_tool_use {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        };
        if !self.message_delta_emitted {
            self.message_delta_emitted = true;
            events.push(sse_bytes(
                "message_delta",
                &json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": stop_reason, "stop_sequence": Value::Null },
                    "usage": usage
                }),
            ));
        }
        events.push(sse_bytes("message_stop", &json!({ "type": "message_stop" })));
        self.terminated = true;
        events
    }

    fn handle_event(&mut self, event_name: &str, value: &Value) -> Vec<Bytes> {
        if self.terminated {
            return Vec::new();
        }

        match event_name {
            "response.created" => {
                let response = value.get("response").unwrap_or(value);
                if let Some(id) = response.get("id").and_then(Value::as_str) {
                    self.message_id = id.to_string();
                }
                if let Some(model) = response.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                let usage = Self::response_usage_to_anthropic(
                    response.get("usage").unwrap_or(&json!({})),
                );
                self.message_started = true;
                vec![sse_bytes(
                    "message_start",
                    &json!({
                        "type": "message_start",
                        "message": {
                            "id": self.message_id,
                            "type": "message",
                            "role": "assistant",
                            "model": self.model,
                            "usage": usage
                        }
                    }),
                )]
            }
            "response.content_part.added" => {
                let part_type = value
                    .get("part")
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str);
                if matches!(part_type, Some("output_text") | Some("refusal")) {
                    let mut events = self.ensure_message_start();
                    events.extend(self.start_text_block());
                    events
                } else {
                    Vec::new()
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let Some(delta) = value.get("delta").and_then(Value::as_str) else {
                    return Vec::new();
                };
                let mut events = self.ensure_message_start();
                events.extend(self.start_text_block());
                if let Some(index) = self.current_text_index {
                    events.push(sse_bytes(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "text_delta", "text": delta }
                        }),
                    ));
                }
                events
            }
            "response.output_text.done" | "response.refusal.done" => self.close_text_block(),
            "response.output_item.added" => {
                let Some(item) = value.get("item") else {
                    return Vec::new();
                };
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let mut events = self.ensure_message_start();
                        events.extend(self.close_text_block());
                        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                        let index = self.next_index();
                        let item_id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| value.get("item_id").and_then(Value::as_str))
                            .unwrap_or("");
                        if !item_id.is_empty() {
                            self.tool_by_item_id.insert(item_id.to_string(), index);
                        }
                        self.tool_by_index.insert(
                            index,
                            ResponsesToolState {
                                id: call_id.to_string(),
                                name: name.to_string(),
                                ..Default::default()
                            },
                        );
                        self.last_tool_index = Some(index);
                        self.has_tool_use = true;
                        events.extend(self.start_tool_block(index, call_id, name));
                        events
                    }
                    Some("reasoning") => {
                        let mut events = self.ensure_message_start();
                        let index = self.next_index();
                        let item_id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| value.get("item_id").and_then(Value::as_str))
                            .unwrap_or("");
                        if !item_id.is_empty() {
                            self.reasoning_by_item_id.insert(item_id.to_string(), index);
                        }
                        self.reasoning_by_index
                            .insert(index, ResponsesReasoningState::default());
                        events.extend(self.start_reasoning_block(index));
                        events
                    }
                    _ => Vec::new(),
                }
            }
            "response.function_call_arguments.delta" => {
                let Some(delta) = value.get("delta").and_then(Value::as_str) else {
                    return Vec::new();
                };
                let mut events = self.ensure_message_start();
                let item_id = value.get("item_id").and_then(Value::as_str);
                let index = item_id
                    .and_then(|id| self.tool_by_item_id.get(id).copied())
                    .or(self.last_tool_index)
                    .unwrap_or_else(|| self.next_index());
                if let Some(id) = item_id {
                    self.tool_by_item_id.insert(id.to_string(), index);
                }
                let id = value.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = value.get("name").and_then(Value::as_str).unwrap_or("");
                let (stored_id, stored_name) = {
                    let state = self.tool_by_index.entry(index).or_default();
                    if !id.is_empty() {
                        state.id = id.to_string();
                    }
                    if !name.is_empty() {
                        state.name = name.to_string();
                    }
                    state.args.push_str(delta);
                    state.had_delta = true;
                    (state.id.clone(), state.name.clone())
                };
                self.last_tool_index = Some(index);
                events.extend(self.start_tool_block(index, &stored_id, &stored_name));
                events.push(sse_bytes(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": { "type": "input_json_delta", "partial_json": delta }
                    }),
                ));
                events
            }
            "response.function_call_arguments.done" => {
                let item_id = value.get("item_id").and_then(Value::as_str);
                let index = item_id
                    .and_then(|id| self.tool_by_item_id.get(id).copied())
                    .or(self.last_tool_index);
                let Some(index) = index else {
                    return Vec::new();
                };
                let mut events = Vec::new();
                if let Some(state) = self.tool_by_index.get_mut(&index) {
                    if !state.had_delta {
                        let args = value
                            .get("arguments")
                            .or_else(|| value.pointer("/item/arguments"))
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                            .unwrap_or(&state.args);
                        if !args.is_empty() {
                            events.push(sse_bytes(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": { "type": "input_json_delta", "partial_json": args }
                                }),
                            ));
                        }
                    }
                }
                events.extend(self.close_tool_block(index));
                if let Some(id) = item_id {
                    self.tool_by_item_id.remove(id);
                }
                self.tool_by_index.remove(&index);
                events
            }
            "response.output_item.done" => {
                let Some(item) = value.get("item") else {
                    return Vec::new();
                };
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let item_id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| value.get("item_id").and_then(Value::as_str));
                        let index = item_id
                            .and_then(|id| self.tool_by_item_id.get(id).copied())
                            .or(self.last_tool_index);
                        let Some(index) = index else {
                            return Vec::new();
                        };
                        let mut events = Vec::new();
                        if let Some(state) = self.tool_by_index.get_mut(&index) {
                            if !state.had_delta {
                                let args = item
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .unwrap_or(&state.args);
                                if !args.is_empty() {
                                    events.push(sse_bytes(
                                        "content_block_delta",
                                        &json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "input_json_delta",
                                                "partial_json": args
                                            }
                                        }),
                                    ));
                                }
                            }
                        }
                        events.extend(self.close_tool_block(index));
                        if let Some(id) = item_id {
                            self.tool_by_item_id.remove(id);
                        }
                        self.tool_by_index.remove(&index);
                        events
                    }
                    Some("reasoning") => {
                        let item_id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| value.get("item_id").and_then(Value::as_str));
                        let index = item_id
                            .and_then(|id| self.reasoning_by_item_id.get(id).copied())
                            .unwrap_or_else(|| self.next_index());
                        let mut events = Vec::new();
                        let full_text = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default();
                        if full_text.is_empty() {
                            if let Some(state) = self.reasoning_by_index.get_mut(&index) {
                                let text = state.text.clone();
                                if !text.is_empty() {
                                    events.extend(self.start_reasoning_block(index));
                                    events.push(sse_bytes(
                                        "content_block_delta",
                                        &json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "thinking_delta",
                                                "thinking": text
                                            }
                                        }),
                                    ));
                                }
                            }
                        } else {
                            self.reasoning_by_index
                                .entry(index)
                                .or_default()
                                .text
                                .push_str(&full_text);
                            events.extend(self.start_reasoning_block(index));
                            events.push(sse_bytes(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": {
                                        "type": "thinking_delta",
                                        "thinking": full_text
                                    }
                                }),
                            ));
                        }
                        events.extend(self.close_reasoning_block(index));
                        if let Some(id) = item_id {
                            self.reasoning_by_item_id.remove(id);
                        }
                        self.reasoning_by_index.remove(&index);
                        events
                    }
                    _ => Vec::new(),
                }
            }
            "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning.delta" => {
                let Some(delta) = value
                    .get("delta")
                    .or_else(|| value.get("text"))
                    .and_then(Value::as_str)
                else {
                    return Vec::new();
                };
                let mut events = self.ensure_message_start();
                let item_id = value.get("item_id").and_then(Value::as_str);
                let index = item_id
                    .and_then(|id| self.reasoning_by_item_id.get(id).copied())
                    .unwrap_or_else(|| self.next_index());
                if let Some(id) = item_id {
                    self.reasoning_by_item_id.insert(id.to_string(), index);
                }
                {
                    self.reasoning_by_index
                        .entry(index)
                        .or_default()
                        .text
                        .push_str(delta);
                }
                events.extend(self.start_reasoning_block(index));
                events.push(sse_bytes(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": { "type": "thinking_delta", "thinking": delta }
                    }),
                ));
                events
            }
            "response.reasoning_summary_text.done"
            | "response.reasoning_text.done"
            | "response.reasoning.done" => {
                let item_id = value.get("item_id").and_then(Value::as_str);
                let index = item_id
                    .and_then(|id| self.reasoning_by_item_id.get(id).copied())
                    .unwrap_or_else(|| self.next_index());
                let mut events = Vec::new();
                let mut text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_default();
                if text.is_empty() {
                    text = self
                        .reasoning_by_index
                        .get(&index)
                        .map(|state| state.text.clone())
                        .unwrap_or_default();
                }
                if !text.is_empty() {
                    self.reasoning_by_index
                        .entry(index)
                        .or_default()
                        .text = text.clone();
                    events.extend(self.start_reasoning_block(index));
                    events.push(sse_bytes(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "thinking_delta", "thinking": text }
                        }),
                    ));
                }
                if event_name == "response.reasoning.done" {
                    events.extend(self.close_reasoning_block(index));
                }
                events
            }
            "response.completed" | "response.incomplete" => {
                let response = value.get("response").unwrap_or(value);
                if matches!(
                    response.get("status").and_then(Value::as_str),
                    Some("failed" | "cancelled")
                ) || response.get("error").is_some_and(|error| !error.is_null())
                {
                    let error = response.get("error").unwrap_or(response);
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                        .unwrap_or("Responses upstream returned a failed terminal response");
                    return self.fail(message, "upstream_error");
                }
                let usage =
                    Self::response_usage_to_anthropic(response.get("usage").unwrap_or(&json!({})));
                let incomplete_reason = if event_name == "response.incomplete"
                    || response.get("status").and_then(Value::as_str) == Some("incomplete")
                {
                    response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .or(Some("max_output_tokens"))
                } else {
                    None
                };
                self.terminal_events(usage, incomplete_reason)
            }
            "response.failed" | "error" => {
                let error = value.get("error").unwrap_or(value);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| error.as_str())
                    .unwrap_or("Responses upstream returned an error");
                self.fail(message, "upstream_error")
            }
            _ => Vec::new(),
        }
    }

    fn finish(&mut self) -> Vec<Bytes> {
        if self.terminated || self.failed {
            return Vec::new();
        }
        let has_open_tool_or_reasoning = self.open_indices.iter().any(|index| {
            self.tool_by_index.contains_key(index) || self.reasoning_by_index.contains_key(index)
        });
        if self.has_substantive_output && !has_open_tool_or_reasoning {
            self.terminal_events(json!({ "input_tokens": 0, "output_tokens": 0 }), Some("max_output_tokens"))
        } else {
            self.fail(
                "Responses upstream stream ended before a terminal event",
                "stream_truncated",
            )
        }
    }

    fn fail(&mut self, message: &str, error_type: &str) -> Vec<Bytes> {
        if self.failed || self.terminated {
            return Vec::new();
        }
        self.failed = true;
        vec![anthropic_error_sse(message, error_type)]
    }

    fn is_terminal(&self) -> bool {
        self.terminated || self.failed
    }
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions SSE -> OpenAI Responses SSE
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OutputItemState {
    output_index: Option<u32>,
    item_id: String,
    text: String,
    added: bool,
    done: bool,
}

#[derive(Default)]
struct ResponsesToolCallState {
    output_index: Option<u32>,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    added: bool,
    done: bool,
}

#[derive(Default)]
struct ChatToResponsesState {
    response_id: String,
    model: String,
    created_at: u64,
    response_started: bool,
    completed: bool,
    failed: bool,
    next_output_index: u32,
    text: OutputItemState,
    reasoning: OutputItemState,
    tools: BTreeMap<usize, ResponsesToolCallState>,
    next_tool_index: usize,
    output_items: Vec<(u32, Value)>,
    latest_usage: Option<Value>,
    finish_reason: Option<String>,
}

impl ChatToResponsesState {
    fn next_output_index(&mut self) -> u32 {
        let index = self.next_output_index;
        self.next_output_index += 1;
        index
    }

    fn base_response(&self, status: &str, output: Vec<Value>) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": output,
            "usage": self.latest_usage.clone().unwrap_or_else(|| {
                json!({
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "total_tokens": 0,
                    "output_tokens_details": { "reasoning_tokens": 0 }
                })
            })
        })
    }

    fn ensure_response_started(&mut self) -> Vec<Bytes> {
        if self.response_started {
            return Vec::new();
        }
        self.response_started = true;
        let response = self.base_response("in_progress", Vec::new());
        vec![
            sse_bytes(
                "response.created",
                &json!({ "type": "response.created", "response": response }),
            ),
            sse_bytes(
                "response.in_progress",
                &json!({ "type": "response.in_progress", "response": response }),
            ),
        ]
    }

    fn push_reasoning(&mut self, delta: &str) -> Vec<Bytes> {
        let mut events = self.ensure_response_started();
        if !self.reasoning.added {
            let output_index = self.next_output_index();
            let item_id = format!("rs_{}", self.response_id);
            self.reasoning.output_index = Some(output_index);
            self.reasoning.item_id = item_id.clone();
            self.reasoning.added = true;
            events.push(sse_bytes(
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "reasoning",
                        "status": "in_progress",
                        "summary": []
                    }
                }),
            ));
            events.push(sse_bytes(
                "response.reasoning_summary_part.added",
                &json!({
                    "type": "response.reasoning_summary_part.added",
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": "" }
                }),
            ));
        }
        self.reasoning.text.push_str(delta);
        if let Some(output_index) = self.reasoning.output_index {
            events.push(sse_bytes(
                "response.reasoning_summary_text.delta",
                &json!({
                    "type": "response.reasoning_summary_text.delta",
                    "item_id": self.reasoning.item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "delta": delta
                }),
            ));
        }
        events
    }

    fn push_text(&mut self, delta: &str) -> Vec<Bytes> {
        let mut events = self.ensure_response_started();
        if !self.text.added {
            let output_index = self.next_output_index();
            let item_id = format!("{}_msg", self.response_id);
            self.text.output_index = Some(output_index);
            self.text.item_id = item_id.clone();
            self.text.added = true;
            events.push(sse_bytes(
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": []
                    }
                }),
            ));
            events.push(sse_bytes(
                "response.content_part.added",
                &json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": "", "annotations": [] }
                }),
            ));
        }
        self.text.text.push_str(delta);
        if let Some(output_index) = self.text.output_index {
            events.push(sse_bytes(
                "response.output_text.delta",
                &json!({
                    "type": "response.output_text.delta",
                    "item_id": self.text.item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": delta
                }),
            ));
        }
        events
    }

    fn flush_ready_tools(&mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        loop {
            let key = self.next_tool_index;
            let Some((call_id, name, arguments)) = self.tools.get(&key).map(|state| {
                (
                    state.call_id.clone(),
                    state.name.clone(),
                    state.arguments.clone(),
                )
            }) else {
                break;
            };
            let added = self.tools.get(&key).map(|state| state.added).unwrap_or(false);
            let done = self.tools.get(&key).map(|state| state.done).unwrap_or(false);
            if added || done {
                self.next_tool_index += 1;
                continue;
            }
            if call_id.is_empty() || name.is_empty() {
                break;
            }

            let output_index = self.next_output_index();
            let item_id = format!("fc_{call_id}");
            let state = self.tools.get_mut(&key).unwrap();
            state.output_index = Some(output_index);
            state.item_id = item_id.clone();
            state.added = true;
            events.push(sse_bytes(
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "function_call",
                        "status": "in_progress",
                        "call_id": call_id,
                        "name": name,
                        "arguments": ""
                    }
                }),
            ));
            if !arguments.is_empty() {
                events.push(sse_bytes(
                    "response.function_call_arguments.delta",
                    &json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": arguments
                    }),
                ));
            }
            self.next_tool_index += 1;
        }
        events
    }

    fn push_tool_calls(&mut self, tool_calls: &[Value]) -> Vec<Bytes> {
        let mut events = self.ensure_response_started();
        for tool_call in tool_calls {
            let index = tool_call
                .get("index")
                .and_then(Value::as_u64)
                .map(|index| index as usize)
                .unwrap_or(self.next_tool_index);
            let function = tool_call.get("function").unwrap_or(&Value::Null);
            if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    self.tools.entry(index).or_default().call_id = id.to_string();
                }
            }
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                if !name.is_empty() {
                    self.tools.entry(index).or_default().name = name.to_string();
                }
            }
            if let Some(args) = function.get("arguments").and_then(Value::as_str) {
                if !args.is_empty() {
                    self.tools.entry(index).or_default().arguments.push_str(args);
                }
            }
            events.extend(self.flush_ready_tools());
        }
        events
    }

    fn close_text(&mut self) -> Vec<Bytes> {
        if !self.text.added || self.text.done {
            return Vec::new();
        }
        self.text.done = true;
        let output_index = self.text.output_index.unwrap_or(0);
        let item_id = self.text.item_id.clone();
        let text = self.text.text.clone();
        let item = json!({
            "id": item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }]
        });
        self.output_items.push((output_index, item.clone()));
        vec![
            sse_bytes(
                "response.output_text.done",
                &json!({
                    "type": "response.output_text.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "text": text
                }),
            ),
            sse_bytes(
                "response.content_part.done",
                &json!({
                    "type": "response.content_part.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": text, "annotations": [] }
                }),
            ),
            sse_bytes(
                "response.output_item.done",
                &json!({
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": item
                }),
            ),
        ]
    }

    fn close_reasoning(&mut self) -> Vec<Bytes> {
        if !self.reasoning.added || self.reasoning.done {
            return Vec::new();
        }
        self.reasoning.done = true;
        let output_index = self.reasoning.output_index.unwrap_or(0);
        let item_id = self.reasoning.item_id.clone();
        let text = self.reasoning.text.clone();
        let item = json!({
            "id": item_id,
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": text }]
        });
        self.output_items.push((output_index, item.clone()));
        vec![
            sse_bytes(
                "response.reasoning_summary_text.done",
                &json!({
                    "type": "response.reasoning_summary_text.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "text": text
                }),
            ),
            sse_bytes(
                "response.reasoning_summary_part.done",
                &json!({
                    "type": "response.reasoning_summary_part.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": text }
                }),
            ),
            sse_bytes(
                "response.output_item.done",
                &json!({
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": item
                }),
            ),
        ]
    }

    fn close_tools(&mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        let keys: Vec<usize> = self.tools.keys().copied().collect();
        for key in keys {
            let (done, added, has_name, call_id, name, arguments) = {
                let Some(state) = self.tools.get(&key) else {
                    continue;
                };
                (
                    state.done,
                    state.added,
                    !state.name.trim().is_empty(),
                    state.call_id.clone(),
                    state.name.clone(),
                    state.arguments.clone(),
                )
            };
            if done {
                continue;
            }
            if !added && !has_name {
                self.tools.get_mut(&key).unwrap().done = true;
                continue;
            }

            let (output_index, item_id) = if !added {
                let output_index = self.next_output_index();
                let used_call_id = if call_id.is_empty() {
                    format!("call_{key}")
                } else {
                    call_id.clone()
                };
                let item_id = format!("fc_{used_call_id}");
                let state = self.tools.get_mut(&key).unwrap();
                state.output_index = Some(output_index);
                state.item_id = item_id.clone();
                state.added = true;
                events.push(sse_bytes(
                    "response.output_item.added",
                    &json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "function_call",
                        "status": "in_progress",
                        "call_id": used_call_id,
                        "name": name,
                        "arguments": ""
                    }
                }),
                ));
                (output_index, item_id)
            } else {
                let state = self.tools.get(&key).unwrap();
                (state.output_index.unwrap_or(0), state.item_id.clone())
            };

            let arguments = canonical_tool_arguments(&arguments);
            let item = json!({
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments
            });
            events.push(sse_bytes(
                "response.function_call_arguments.done",
                &json!({
                    "type": "response.function_call_arguments.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "arguments": arguments
                }),
            ));
            events.push(sse_bytes(
                "response.output_item.done",
                &json!({
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": item
                }),
            ));
            self.output_items.push((output_index, item));
            self.tools.get_mut(&key).unwrap().done = true;
        }
        events
    }

    fn handle_event(&mut self, event_name: &str, value: &Value) -> Vec<Bytes> {
        if event_name == "error" || value.get("error").is_some() {
            let error = value.get("error").unwrap_or(value);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("OpenAI Chat stream returned an error");
            return self.fail(message, "upstream_error");
        }
        if event_name != "" && event_name != "chat.completion.chunk" {
            return Vec::new();
        }

        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.response_id = if id.starts_with("resp_") {
                id.to_string()
            } else {
                format!("resp_{id}")
            };
        }
        if self.response_id.is_empty() {
            self.response_id = "resp_ccswitch".to_string();
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            if !model.is_empty() {
                self.model = model.to_string();
            }
        }
        if let Some(created) = value.get("created").and_then(Value::as_u64) {
            self.created_at = created;
        }
        if let Some(usage) = value.get("usage").filter(|value| value.is_object()) {
            self.latest_usage = Some(usage_shape::openai_chat_usage_to_responses(Some(usage)));
        }

        let mut events = Vec::new();
        if let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = chat_delta_reasoning(delta) {
                    events.extend(self.push_reasoning(&reasoning));
                }
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        events.extend(self.push_text(content));
                    }
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    events.extend(self.push_tool_calls(tool_calls));
                }
            }
            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                if self.finish_reason.is_none() {
                    self.finish_reason = Some(finish_reason.to_string());
                }
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<Bytes> {
        if self.completed || self.failed {
            return Vec::new();
        }
        let mut events = self.ensure_response_started();
        events.extend(self.close_reasoning());
        events.extend(self.close_text());
        events.extend(self.close_tools());

        let has_output = !self.output_items.is_empty()
            || self.text.added
            || self.reasoning.added
            || self.tools.values().any(|tool| {
                !tool.call_id.is_empty() || !tool.name.is_empty() || !tool.arguments.is_empty()
            });
        let status = match self.finish_reason.as_deref() {
            Some("length") => "incomplete",
            Some(_) => "completed",
            None if has_output => "incomplete",
            None => {
                events.push(responses_failed_sse(
                    "OpenAI Chat stream ended before sending finish_reason",
                    "stream_truncated",
                ));
                self.failed = true;
                return events;
            }
        };

        let mut output_items = self.output_items.clone();
        output_items.sort_by_key(|(index, _)| *index);
        let output: Vec<Value> = output_items.into_iter().map(|(_, item)| item).collect();
        let mut response = self.base_response(status, output);
        if status == "incomplete" {
            response["incomplete_details"] = json!({ "reason": "max_output_tokens" });
        }
        events.push(sse_bytes(
            "response.completed",
            &json!({ "type": "response.completed", "response": response }),
        ));
        self.completed = true;
        events
    }

    fn fail(&mut self, message: &str, error_type: &str) -> Vec<Bytes> {
        if self.failed || self.completed {
            return Vec::new();
        }
        self.failed = true;
        vec![responses_failed_sse(message, error_type)]
    }

    fn is_terminal(&self) -> bool {
        self.completed || self.failed
    }
}

// ---------------------------------------------------------------------------
// Anthropic Messages SSE -> OpenAI Responses SSE
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum AnthropicBlockKind {
    #[default]
    Text,
    Tool,
    Thinking,
}

#[derive(Default)]
struct AnthropicBlockState {
    kind: AnthropicBlockKind,
    output_index: u32,
    item_id: String,
    call_id: String,
    name: String,
    text: String,
    done: bool,
}

#[derive(Default)]
struct AnthropicToResponsesState {
    response_id: String,
    model: String,
    next_output_index: u32,
    response_started: bool,
    completed: bool,
    failed: bool,
    blocks: BTreeMap<u64, AnthropicBlockState>,
    usage: Value,
    stop_reason: Option<String>,
    output_items: Vec<(u32, Value)>,
}

impl AnthropicToResponsesState {
    fn next_output_index(&mut self) -> u32 {
        let index = self.next_output_index;
        self.next_output_index += 1;
        index
    }

    fn responses_usage(&self) -> Value {
        usage_shape::anthropic_usage_to_responses(Some(&self.usage))
    }

    fn base_response(&self, status: &str, output: Vec<Value>) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": 0,
            "status": status,
            "model": self.model,
            "output": output,
            "usage": self.responses_usage()
        })
    }

    fn ensure_response_started(&mut self) -> Vec<Bytes> {
        if self.response_started {
            return Vec::new();
        }
        self.response_started = true;
        let response = self.base_response("in_progress", Vec::new());
        vec![
            sse_bytes(
                "response.created",
                &json!({ "type": "response.created", "response": response }),
            ),
            sse_bytes(
                "response.in_progress",
                &json!({ "type": "response.in_progress", "response": response }),
            ),
        ]
    }

    fn merge_usage(&mut self, usage: &Value) {
        if let Some(obj) = usage.as_object() {
            if self.usage.is_null() {
                self.usage = json!({});
            }
            let Some(target) = self.usage.as_object_mut() else {
                return;
            };
            for (key, value) in obj {
                if !value.is_null() {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }

    fn handle_event(&mut self, event_name: &str, value: &Value) -> Vec<Bytes> {
        match event_name {
            "message_start" => {
                if let Some(message) = value.get("message") {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        self.response_id = if id.starts_with("resp_") {
                            id.to_string()
                        } else {
                            format!("resp_{id}")
                        };
                    }
                    if let Some(model) = message.get("model").and_then(Value::as_str) {
                        if !model.is_empty() {
                            self.model = model.to_string();
                        }
                    }
                    if let Some(usage) = message.get("usage") {
                        self.merge_usage(usage);
                    }
                }
                self.ensure_response_started()
            }
            "content_block_start" => {
                let mut events = self.ensure_response_started();
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    return events;
                };
                let block = value.get("content_block").unwrap_or(&Value::Null);
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                let output_index = self.next_output_index();
                match block_type {
                    "text" => {
                        let item_id = format!("{}_msg_{output_index}", self.response_id);
                        events.push(sse_bytes(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "message",
                                    "status": "in_progress",
                                    "role": "assistant",
                                    "content": []
                                }
                            }),
                        ));
                        events.push(sse_bytes(
                            "response.content_part.added",
                            &json!({
                                "type": "response.content_part.added",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "part": { "type": "output_text", "text": "", "annotations": [] }
                            }),
                        ));
                        self.blocks.insert(
                            index,
                            AnthropicBlockState {
                                kind: AnthropicBlockKind::Text,
                                output_index,
                                item_id,
                                text: block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                ..Default::default()
                            },
                        );
                    }
                    "tool_use" => {
                        let call_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        let item_id = format!("fc_{call_id}");
                        events.push(sse_bytes(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "status": "in_progress",
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": ""
                                }
                            }),
                        ));
                        let mut text = String::new();
                        if let Some(input) = block.get("input") {
                            if !input.is_null() {
                                text = canonical_tool_arguments(&input.to_string());
                            }
                        }
                        self.blocks.insert(
                            index,
                            AnthropicBlockState {
                                kind: AnthropicBlockKind::Tool,
                                output_index,
                                item_id,
                                call_id: call_id.to_string(),
                                name: name.to_string(),
                                text,
                                ..Default::default()
                            },
                        );
                    }
                    "thinking" | "redacted_thinking" => {
                        let item_id = format!("rs_{}_{output_index}", self.response_id);
                        events.push(sse_bytes(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "reasoning",
                                    "status": "in_progress",
                                    "summary": []
                                }
                            }),
                        ));
                        if block_type == "thinking" {
                            events.push(sse_bytes(
                                "response.reasoning_summary_part.added",
                                &json!({
                                    "type": "response.reasoning_summary_part.added",
                                    "item_id": item_id,
                                    "output_index": output_index,
                                    "summary_index": 0,
                                    "part": { "type": "summary_text", "text": "" }
                                }),
                            ));
                        }
                        self.blocks.insert(
                            index,
                            AnthropicBlockState {
                                kind: AnthropicBlockKind::Thinking,
                                output_index,
                                item_id,
                                text: block
                                    .get("thinking")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                ..Default::default()
                            },
                        );
                    }
                    _ => {}
                }
                events
            }
            "content_block_delta" => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let Some(delta) = value.get("delta") else {
                    return Vec::new();
                };
                let Some(block) = self.blocks.get_mut(&index) else {
                    return Vec::new();
                };
                let output_index = block.output_index;
                let item_id = block.item_id.clone();
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        block.text.push_str(text);
                        vec![sse_bytes(
                            "response.output_text.delta",
                            &json!({
                                "type": "response.output_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "delta": text
                            }),
                        )]
                    }
                    Some("input_json_delta") => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        block.text.push_str(partial);
                        vec![sse_bytes(
                            "response.function_call_arguments.delta",
                            &json!({
                                "type": "response.function_call_arguments.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "delta": partial
                            }),
                        )]
                    }
                    Some("thinking_delta") => {
                        let thinking = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        block.text.push_str(thinking);
                        vec![sse_bytes(
                            "response.reasoning_summary_text.delta",
                            &json!({
                                "type": "response.reasoning_summary_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "delta": thinking
                            }),
                        )]
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                self.close_block(index)
            }
            "message_delta" => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = value.get("usage") {
                    self.merge_usage(usage);
                }
                Vec::new()
            }
            "message_stop" => self.finish(),
            "error" => {
                let error = value.get("error").unwrap_or(value);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| error.as_str())
                    .unwrap_or("Anthropic upstream returned an error");
                self.fail(message, "upstream_error")
            }
            _ => Vec::new(),
        }
    }

    fn close_block(&mut self, index: u64) -> Vec<Bytes> {
        let Some(block) = self.blocks.get_mut(&index) else {
            return Vec::new();
        };
        if block.done {
            return Vec::new();
        }
        block.done = true;
        let output_index = block.output_index;
        let item_id = block.item_id.clone();
        let kind = block.kind;
        let text = block.text.clone();
        let call_id = block.call_id.clone();
        let name = block.name.clone();

        let mut events = Vec::new();
        match kind {
            AnthropicBlockKind::Text => {
                let item = json!({
                    "id": item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text, "annotations": [] }]
                });
                events.extend([
                    sse_bytes(
                        "response.output_text.done",
                        &json!({
                            "type": "response.output_text.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "text": text
                        }),
                    ),
                    sse_bytes(
                        "response.content_part.done",
                        &json!({
                            "type": "response.content_part.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "part": { "type": "output_text", "text": text, "annotations": [] }
                        }),
                    ),
                    sse_bytes(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": item
                        }),
                    ),
                ]);
                self.output_items.push((output_index, item));
            }
            AnthropicBlockKind::Tool => {
                let arguments = canonical_tool_arguments(&text);
                let item = json!({
                    "id": item_id,
                    "type": "function_call",
                    "status": "completed",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                });
                events.push(sse_bytes(
                    "response.function_call_arguments.done",
                    &json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments
                    }),
                ));
                events.push(sse_bytes(
                    "response.output_item.done",
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item
                    }),
                ));
                self.output_items.push((output_index, item));
            }
            AnthropicBlockKind::Thinking => {
                let item = json!({
                    "id": item_id,
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": text }]
                });
                events.extend([
                    sse_bytes(
                        "response.reasoning_summary_text.done",
                        &json!({
                            "type": "response.reasoning_summary_text.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "text": text
                        }),
                    ),
                    sse_bytes(
                        "response.reasoning_summary_part.done",
                        &json!({
                            "type": "response.reasoning_summary_part.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": text }
                        }),
                    ),
                    sse_bytes(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": item
                        }),
                    ),
                ]);
                self.output_items.push((output_index, item));
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<Bytes> {
        if self.completed || self.failed {
            return Vec::new();
        }
        let mut events = self.ensure_response_started();
        let open: Vec<u64> = self
            .blocks
            .iter()
            .filter(|(_, block)| !block.done)
            .map(|(index, _)| *index)
            .collect();
        for index in open {
            events.extend(self.close_block(index));
        }

        let (status, incomplete_reason) = match self.stop_reason.as_deref() {
            Some("max_tokens") => ("incomplete", Some("max_output_tokens")),
            Some("refusal") => ("incomplete", Some("content_filter")),
            Some("model_context_window_exceeded") => ("incomplete", Some("max_output_tokens")),
            _ => ("completed", None),
        };
        let mut output_items = self.output_items.clone();
        output_items.sort_by_key(|(index, _)| *index);
        let output: Vec<Value> = output_items.into_iter().map(|(_, item)| item).collect();
        let mut response = self.base_response(status, output);
        if let Some(reason) = incomplete_reason {
            response["incomplete_details"] = json!({ "reason": reason });
        }
        events.push(sse_bytes(
            "response.completed",
            &json!({ "type": "response.completed", "response": response }),
        ));
        self.completed = true;
        events
    }

    fn fail(&mut self, message: &str, error_type: &str) -> Vec<Bytes> {
        if self.failed || self.completed {
            return Vec::new();
        }
        self.failed = true;
        vec![responses_failed_sse(message, error_type)]
    }

    fn is_terminal(&self) -> bool {
        self.completed || self.failed
    }
}

// ---------------------------------------------------------------------------
// Non-streaming JSON fallback: emit the same client SSE lifecycle.
// ---------------------------------------------------------------------------

pub(crate) fn json_response_to_sse(client: &str, value: &Value) -> Result<Vec<Bytes>, String> {
    match client {
        "anthropic" => Ok(anthropic_message_to_sse(value)),
        "openai_responses" => Ok(responses_object_to_sse(value)),
        _ => Err(format!("unsupported streaming client format: {client}")),
    }
}

fn anthropic_message_to_sse(message: &Value) -> Vec<Bytes> {
    let mut events = vec![sse_bytes(
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message.get("id").cloned().unwrap_or_else(|| json!("")),
                "type": "message",
                "role": "assistant",
                "model": message.get("model").cloned().unwrap_or_else(|| json!("")),
                "usage": {
                    "input_tokens": message
                        .pointer("/usage/input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    "output_tokens": 0
                }
            }
        }),
    )];

    if let Some(content) = message.get("content").and_then(Value::as_array) {
        for (index, block) in content.iter().enumerate() {
            let index = index as u64;
            let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
            match block_type {
                "text" => {
                    events.push(sse_bytes(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": { "type": "text", "text": "" }
                        }),
                    ));
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            events.push(sse_bytes(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": { "type": "text_delta", "text": text }
                                }),
                            ));
                        }
                    }
                    events.push(sse_bytes(
                        "content_block_stop",
                        &json!({ "type": "content_block_stop", "index": index }),
                    ));
                }
                "tool_use" => {
                    events.push(sse_bytes(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": {
                                "type": "tool_use",
                                "id": block.get("id").cloned().unwrap_or_else(|| json!("")),
                                "name": block.get("name").cloned().unwrap_or_else(|| json!("")),
                                "input": {}
                            }
                        }),
                    ));
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    events.push(sse_bytes(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": canonical_tool_arguments(&input.to_string())
                            }
                        }),
                    ));
                    events.push(sse_bytes(
                        "content_block_stop",
                        &json!({ "type": "content_block_stop", "index": index }),
                    ));
                }
                "thinking" => {
                    events.push(sse_bytes(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": { "type": "thinking", "thinking": "" }
                        }),
                    ));
                    if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                        if !text.is_empty() {
                            events.push(sse_bytes(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": { "type": "thinking_delta", "thinking": text }
                                }),
                            ));
                        }
                    }
                    events.push(sse_bytes(
                        "content_block_stop",
                        &json!({ "type": "content_block_stop", "index": index }),
                    ));
                }
                _ => {}
            }
        }
    }

    let usage = message
        .get("usage")
        .cloned()
        .unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }));
    events.push(sse_bytes(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": message
                    .get("stop_reason")
                    .cloned()
                    .unwrap_or_else(|| json!("end_turn")),
                "stop_sequence": Value::Null
            },
            "usage": usage
        }),
    ));
    events.push(sse_bytes("message_stop", &json!({ "type": "message_stop" })));
    events
}

fn responses_object_to_sse(response: &Value) -> Vec<Bytes> {
    let mut events = vec![
        sse_bytes(
            "response.created",
            &json!({ "type": "response.created", "response": response }),
        ),
        sse_bytes(
            "response.in_progress",
            &json!({ "type": "response.in_progress", "response": response }),
        ),
    ];

    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for (output_index, item) in output.iter().enumerate() {
            let output_index = output_index as u32;
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(&format!("msg_{output_index}"))
                        .to_string();
                    let text = item
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| {
                                    part.get("text").and_then(Value::as_str)
                                })
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    events.push(sse_bytes(
                        "response.output_item.added",
                        &json!({
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": item_id,
                                "type": "message",
                                "status": "in_progress",
                                "role": "assistant",
                                "content": []
                            }
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.content_part.added",
                        &json!({
                            "type": "response.content_part.added",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "part": { "type": "output_text", "text": "", "annotations": [] }
                        }),
                    ));
                    if !text.is_empty() {
                        events.push(sse_bytes(
                            "response.output_text.delta",
                            &json!({
                                "type": "response.output_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "delta": text
                            }),
                        ));
                    }
                    events.push(sse_bytes(
                        "response.output_text.done",
                        &json!({
                            "type": "response.output_text.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "text": text
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.content_part.done",
                        &json!({
                            "type": "response.content_part.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "part": { "type": "output_text", "text": text, "annotations": [] }
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": item
                        }),
                    ));
                }
                Some("function_call") => {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(&format!("fc_{output_index}"))
                        .to_string();
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string();
                    events.push(sse_bytes(
                        "response.output_item.added",
                        &json!({
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": item_id,
                                "type": "function_call",
                                "status": "in_progress",
                                "call_id": item
                                    .get("call_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                                "name": item
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                                "arguments": ""
                            }
                        }),
                    ));
                    if !arguments.is_empty() {
                        events.push(sse_bytes(
                            "response.function_call_arguments.delta",
                            &json!({
                                "type": "response.function_call_arguments.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "delta": arguments
                            }),
                        ));
                    }
                    events.push(sse_bytes(
                        "response.function_call_arguments.done",
                        &json!({
                            "type": "response.function_call_arguments.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "arguments": arguments
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": item
                        }),
                    ));
                }
                Some("reasoning") => {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(&format!("rs_{output_index}"))
                        .to_string();
                    let text = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    events.push(sse_bytes(
                        "response.output_item.added",
                        &json!({
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": item_id,
                                "type": "reasoning",
                                "status": "in_progress",
                                "summary": []
                            }
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.reasoning_summary_part.added",
                        &json!({
                            "type": "response.reasoning_summary_part.added",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": "" }
                        }),
                    ));
                    if !text.is_empty() {
                        events.push(sse_bytes(
                            "response.reasoning_summary_text.delta",
                            &json!({
                                "type": "response.reasoning_summary_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "delta": text
                            }),
                        ));
                    }
                    events.push(sse_bytes(
                        "response.reasoning_summary_text.done",
                        &json!({
                            "type": "response.reasoning_summary_text.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "text": text
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.reasoning_summary_part.done",
                        &json!({
                            "type": "response.reasoning_summary_part.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": text }
                        }),
                    ));
                    events.push(sse_bytes(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": item
                        }),
                    ));
                }
                _ => {}
            }
        }
    }

    let mut final_response = response.clone();
    if final_response.get("status").is_none() {
        final_response["status"] = json!("completed");
    }
    events.push(sse_bytes(
        "response.completed",
        &json!({ "type": "response.completed", "response": final_response }),
    ));
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};

    async fn collect_stream(mut stream: ConvertingStream) -> String {
        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8_lossy(&item.unwrap()));
        }
        output
    }

    fn converting(client: &str, upstream: &str, input: &str) -> ConvertingStream {
        let input_stream: Pin<Box<dyn Stream<Item = Result<Bytes, UpstreamError>> + Send>> =
            Box::pin(stream::iter(vec![Ok::<_, UpstreamError>(Bytes::from(
                input.as_bytes().to_vec(),
            ))]));
        ConvertingStream::new(client, upstream, input_stream).unwrap()
    }

    #[tokio::test]
    async fn chat_sse_to_anthropic_sse() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        let output = collect_stream(converting("anthropic", "openai_chat", input)).await;
        assert!(output.contains("event: message_start"));
        // 分开断言：上游原测试把两个键写在一个子串里，依赖 serde_json 的键序。
        // CCProxy 开了 preserve_order（配置要按原顺序落盘），键序与上游不同，JSON 语义不受影响
        assert!(output.contains("\"text\":\"Hel\"") && output.contains("\"type\":\"text_delta\""));
        assert!(output.contains("\"text\":\"lo\"") && output.contains("\"type\":\"text_delta\""));
        assert!(output.contains("\"stop_reason\":\"end_turn\""));
        assert!(output.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn chat_sse_to_responses_sse() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"created\":1,\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"created\":1,\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        let output = collect_stream(converting("openai_responses", "openai_chat", input)).await;
        assert!(output.contains("event: response.created"));
        assert!(output.contains("\"id\":\"resp_chatcmpl_1\""));
        assert!(output.contains("event: response.output_text.delta"));
        assert!(output.contains("\"delta\":\"hi\""));
        assert!(output.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn responses_sse_to_anthropic_sse() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5\",\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\",\"output_index\":0,\"content_index\":0}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n"
        );
        let output = collect_stream(converting("anthropic", "openai_responses", input)).await;
        assert!(output.contains("event: message_start"));
        assert!(output.contains("\"text\":\"ok\"") && output.contains("\"type\":\"text_delta\""));
        assert!(output.contains("\"stop_reason\":\"end_turn\""));
        assert!(output.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn anthropic_sse_to_responses_sse() {
        let input = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        let output = collect_stream(converting("openai_responses", "anthropic", input)).await;
        assert!(output.contains("event: response.created"));
        assert!(output.contains("\"id\":\"resp_msg_1\""));
        assert!(output.contains("event: response.output_text.delta"));
        assert!(output.contains("\"delta\":\"hi\""));
        assert!(output.contains("event: response.completed"));
    }
}
