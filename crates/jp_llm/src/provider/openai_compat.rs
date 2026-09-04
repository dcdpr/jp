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
//! Deserialization is deliberately lenient: every field defaults, and unknown
//! ones are ignored, so a provider adding a field to a chunk does not break the
//! stream.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
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
