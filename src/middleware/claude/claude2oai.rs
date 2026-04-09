use std::sync::{Arc, Mutex};

use axum::response::sse::Event;
use futures::{Stream, TryStreamExt};
use serde::Serialize;
use serde_json::Value;

use crate::types::claude::{ContentBlockDelta, CreateMessageResponse, StopReason, StreamEvent};

/// Represents the data structure for streaming events in OpenAI API format
/// Contains a choices array with deltas of content
#[derive(Debug, Serialize)]
struct StreamEventData {
    choices: Vec<StreamEventDelta>,
}

impl StreamEventData {
    /// Creates a new StreamEventData with the given content
    ///
    /// # Arguments
    /// * `content` - The event content to include
    ///
    /// # Returns
    /// A new StreamEventData instance with the content wrapped in choices array
    fn new(content: EventContent) -> Self {
        Self {
            choices: vec![StreamEventDelta { delta: content }],
        }
    }
}

/// Represents a delta update in a streaming response
/// Contains the content change for the current chunk
#[derive(Debug, Serialize)]
struct StreamEventDelta {
    delta: EventContent,
}

/// Content of an event, either regular content or reasoning (thinking mode)
/// Uses untagged enum to handle different response formats
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum EventContent {
    Content { content: String },
    Reasoning { reasoning_content: String },
}

/// Creates an SSE event with the given content in OpenAI format
///
/// # Arguments
/// * `content` - The event content to include
///
/// # Returns
/// A formatted SSE Event ready to be sent to the client
pub fn build_event(content: EventContent) -> Event {
    let event = Event::default();
    let data = StreamEventData::new(content);
    event.json_data(data).unwrap()
}

/// Accumulates metadata from a streaming Claude response so we can emit
/// a proper OpenAI-format final chunk with `finish_reason` and `usage`.
#[derive(Default)]
struct StreamState {
    id: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_creation_tokens: u32,
    cache_read_tokens: u32,
    finish_reason: String,
}

impl StreamState {
    fn new() -> Self {
        Self {
            finish_reason: "stop".to_string(),
            ..Default::default()
        }
    }

    fn stop_reason_str(reason: &StopReason) -> &'static str {
        match reason {
            StopReason::EndTurn => "stop",
            StopReason::MaxTokens => "length",
            StopReason::StopSequence => "stop",
            StopReason::ToolUse => "tool_calls",
            StopReason::PauseTurn => "stop",
            StopReason::Refusal => "content_filter",
            StopReason::ModelContextWindowExceeded => "length",
        }
    }

    /// Build the final SSE chunk that carries `finish_reason` + `usage`,
    /// matching the OpenAI streaming format expected by aggregators such as CLIProxyAPI.
    /// Includes cache token counts so downstream stats tools show correct numbers.
    fn final_chunk(&self) -> Event {
        let mut usage = serde_json::json!({
            "prompt_tokens": self.input_tokens,
            "completion_tokens": self.output_tokens,
            "total_tokens": self.input_tokens + self.output_tokens,
            // Anthropic-style fields (read by CLIProxyAPI and similar tools)
            "cache_creation_input_tokens": self.cache_creation_tokens,
            "cache_read_input_tokens": self.cache_read_tokens,
        });
        // OpenAI-style nested field for clients that prefer it
        if self.cache_read_tokens > 0 {
            usage["prompt_tokens_details"] = serde_json::json!({
                "cached_tokens": self.cache_read_tokens
            });
        }
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            "model": self.model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": self.finish_reason}],
            "usage": usage
        });
        Event::default().json_data(chunk).unwrap()
    }
}

/// Transforms a Claude.ai event stream into an OpenAI-compatible event stream.
///
/// Previously this function only forwarded `ContentBlockDelta` events, which meant
/// downstream aggregators (e.g. CLIProxyAPI) never received token-usage data and
/// reported 0 for all token counts and latency.
///
/// Now it additionally:
/// - Captures `input_tokens` from `MessageStart`
/// - Accumulates `output_tokens` and `stop_reason` from `MessageDelta`
/// - Emits a final chunk on `MessageStop` with `finish_reason` and `usage`
///   in the format OpenAI streaming clients expect.
///
/// # Type Parameters
/// * `I` - The input stream type
/// * `E` - The error type for the stream
pub fn transform_stream<I, E>(s: I) -> impl Stream<Item = Result<Event, E>>
where
    I: Stream<Item = Result<eventsource_stream::Event, E>>,
{
    let state = Arc::new(Mutex::new(StreamState::new()));

    s.try_filter_map(move |eventsource_stream::Event { data, .. }| {
        let state = Arc::clone(&state);
        async move {
            let Ok(parsed) = serde_json::from_str::<StreamEvent>(&data) else {
                return Ok(None);
            };

            match parsed {
                // Capture id, model, input token count, and cache stats from the opening event
                StreamEvent::MessageStart { message } => {
                    let mut st = state.lock().unwrap();
                    st.id = message.id;
                    st.model = message.model;
                    if let Some(usage) = message.usage {
                        st.input_tokens = usage.input_tokens;
                        st.cache_creation_tokens =
                            usage.cache_creation_input_tokens.unwrap_or(0);
                        st.cache_read_tokens = usage.cache_read_input_tokens.unwrap_or(0);
                    }
                    Ok(None)
                }

                // Accumulate output tokens and the stop reason
                StreamEvent::MessageDelta { delta, usage } => {
                    let mut st = state.lock().unwrap();
                    if let Some(u) = usage {
                        st.output_tokens += u.output_tokens;
                    }
                    if let Some(reason) = delta.stop_reason {
                        st.finish_reason = StreamState::stop_reason_str(&reason).to_string();
                    }
                    Ok(None)
                }

                // Emit the final chunk with finish_reason + usage when the message ends
                StreamEvent::MessageStop => {
                    let st = state.lock().unwrap();
                    Ok(Some(st.final_chunk()))
                }

                // Forward text and reasoning deltas as before
                StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                    ContentBlockDelta::TextDelta { text } => {
                        Ok(Some(build_event(EventContent::Content { content: text })))
                    }
                    ContentBlockDelta::ThinkingDelta { thinking } => {
                        Ok(Some(build_event(EventContent::Reasoning {
                            reasoning_content: thinking,
                        })))
                    }
                    _ => Ok(None),
                },

                _ => Ok(None),
            }
        }
    })
}

pub fn transforms_json(input: CreateMessageResponse) -> Value {
    let content = input
        .content
        .iter()
        .filter_map(|block| match block {
            crate::types::claude::ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<String>();

    let usage = input.usage.as_ref().map(|u| {
        let cache_creation = u.cache_creation_input_tokens.unwrap_or(0);
        let cache_read = u.cache_read_input_tokens.unwrap_or(0);
        let mut obj = serde_json::json!({
            "prompt_tokens": u.input_tokens,
            "completion_tokens": u.output_tokens,
            "total_tokens": u.input_tokens + u.output_tokens,
            "cache_creation_input_tokens": cache_creation,
            "cache_read_input_tokens": cache_read,
        });
        if cache_read > 0 {
            obj["prompt_tokens_details"] = serde_json::json!({
                "cached_tokens": cache_read
            });
        }
        obj
    });

    let finish_reason = match input.stop_reason {
        Some(crate::types::claude::StopReason::EndTurn) => "stop",
        Some(crate::types::claude::StopReason::MaxTokens) => "length",
        Some(crate::types::claude::StopReason::StopSequence) => "stop",
        Some(crate::types::claude::StopReason::ToolUse) => "tool_calls",
        Some(crate::types::claude::StopReason::PauseTurn) => "stop",
        Some(crate::types::claude::StopReason::Refusal) => "content_filter",
        Some(crate::types::claude::StopReason::ModelContextWindowExceeded) => "length",
        None => "stop",
    };

    serde_json::json!({
        "id": input.id,
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "model": input.model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": content
            },
            "finish_reason": finish_reason
        }],
        "usage": usage
    })
}
