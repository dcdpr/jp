//! The OpenAI chat-completions wire format, shared by the providers that speak
//! it.
//!
//! This is the `/v1/chat/completions` dialect: SSE chunks carrying
//! `choices[].delta`, terminated by a `[DONE]` sentinel.
//! The `llamacpp` and `cerebras` providers both stream it.
//! It is a different protocol from the OpenAI Responses API that the `openai`
//! provider uses, which has its own typed event enum.
//!
//! The chunk types mirror llama.cpp's `common_chat_msg_diff_to_json_oaicompat`
//! output.
//! Their addition over the plain OpenAI shape is `reasoning_content`, which
//! carries extracted reasoning for llama.cpp's `--reasoning-format deepseek`
//! (default) and `deepseek-legacy` modes; Cerebras spells the same field
//! `reasoning`, and a serde alias accepts both.
//!
//! Deserialization is deliberately lenient: unknown fields are ignored and the
//! optional fields default, so a provider adding a field to a chunk does not
//! break the stream.
//! `StreamChoice::delta` is the one required field; a chunk whose choice omits
//! it fails to parse, and both providers log a warning and skip that chunk.

use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, warn};

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,

    /// An error the provider reported inside the stream, once the response had
    /// already returned 200.
    ///
    /// Held as raw JSON because nothing reads it.
    /// Every failure these providers are known to produce arrives as an HTTP
    /// status before the stream opens, so this field exists for [`parse_chunk`]
    /// to say something out loud if that ever stops being true.
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: StreamDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct StreamDelta {
    #[serde(default)]
    pub content: Option<String>,
    /// Reasoning content extracted by the server (deepseek / deepseek-legacy).
    /// This is a non-standard `DeepSeek` extension that llama.cpp also uses.
    #[serde(default, alias = "reasoning")]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Parse one SSE `data:` payload from `provider` into a chunk.
///
/// Returns `None` when the payload yields nothing to emit, having logged the
/// reason.
pub(crate) fn parse_chunk(data: &str, provider: &str) -> Option<StreamChunk> {
    let chunk: StreamChunk = match serde_json::from_str(data) {
        Ok(chunk) => chunk,
        Err(error) => {
            warn!(provider, %error, data, "Failed to parse chunk.");
            return None;
        }
    };

    // Nothing downstream can act on this, so at least record it. A stream that
    // reports a failure this way and then stops has sent no terminal event,
    // which reads to the retry layer as a dropped connection: it resends the
    // request blind, several times, and the user is told the connection failed.
    if let Some(error) = &chunk.error {
        warn!(
            provider,
            error = %error,
            "Provider reported an error inside the stream. It will surface as a \
             truncated response instead of this message."
        );
        return None;
    }

    if chunk.choices.is_empty() {
        debug!(provider, data, "Chunk carried no choices.");
        return None;
    }

    Some(chunk)
}

/// Merge consecutive assistant messages in an OpenAI-compatible
/// chat-completions message list into single messages.
///
/// A single model turn can surface reasoning, content, and several parallel
/// tool calls as separate events.
/// The chat-completions contract expects them as one assistant message:
/// reasoning and content folded in, every parallel `tool_calls` entry in one
/// array, immediately followed by the tool results.
///
/// Both the `reasoning` (Cerebras) and `reasoning_content` (llama.cpp) field
/// names are handled.
pub(crate) fn merge_consecutive_assistant_messages(messages: Vec<Value>) -> Vec<Value> {
    messages
        .into_iter()
        .fold(vec![], |mut acc: Vec<Value>, message| {
            if let Some(last) = acc.last_mut()
                && last.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("role").and_then(Value::as_str) == Some("assistant")
            {
                if let Some(new) = message.get("tool_calls").and_then(Value::as_array) {
                    last["tool_calls"]
                        .as_array_mut()
                        .map(|existing| existing.extend(new.iter().cloned()))
                        .unwrap_or_else(|| last["tool_calls"] = json!(new));
                }

                for key in ["reasoning", "reasoning_content", "content"] {
                    if let Some(value) = message.get(key)
                        && value.is_string()
                    {
                        last[key] = value.clone();
                    }
                }

                return acc;
            }

            acc.push(message);
            acc
        })
}

#[cfg(test)]
#[path = "openai_compat_tests.rs"]
mod tests;
