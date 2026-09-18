//! Qualified Claude SDK options assembled from a resolved JP request.

use std::collections::BTreeMap;

use async_anthropic::types::{Effort, ExtendedThinking, JsonOutputFormat, ThinkingDisplay};
use jp_config::{assistant::request::CachePolicy, model::id::Name};
use serde::Serialize;
use serde_json::Value;

use super::transcript::PreparedRequest;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Extension<'a> {
    claude_code: ClaudeCode<'a>,
}

#[derive(Serialize)]
struct ClaudeCode<'a> {
    #[serde(rename = "emitRawSDKMessages")]
    emit_raw_sdk_messages: bool,
    options: Options<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Options<'a> {
    system_prompt: CustomPrompt<'a>,
    model: &'a Name,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking>,
    tools: [(); 0],
    allowed_tools: [(); 0],
    strict_mcp_config: bool,
    setting_sources: [(); 0],
    settings: Settings,
    persist_session: bool,
    env: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<&'a JsonOutputFormat>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CustomPrompt<'a> {
    Custom { prompt: &'a str, snapshot: bool },
}

/// Claude Code's `ThinkingConfig`.
///
/// Structurally identical to [`ExtendedThinking`] apart from `budgetTokens`,
/// which the SDK spells in camelCase where the Anthropic API uses
/// `budget_tokens`.
/// That single difference is the only reason this type exists.
///
/// `display` carries more weight than its size suggests.
/// Opus 4.7 and later default it to `omitted`, which leaves the model reasoning
/// and billing for tokens that arrive as empty thinking blocks.
#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Thinking {
    Enabled {
        budget_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    Disabled,
}

impl From<&ExtendedThinking> for Thinking {
    // Every variant is destructured field by field, with no `..` rest pattern:
    // a field added to `ExtendedThinking` has to fail the build here instead of
    // being dropped silently on the way to Claude Code.
    fn from(thinking: &ExtendedThinking) -> Self {
        match thinking {
            ExtendedThinking::Enabled {
                budget_tokens,
                display,
            } => Self::Enabled {
                budget_tokens: *budget_tokens,
                display: display.clone(),
            },
            ExtendedThinking::Adaptive { display } => Self::Adaptive {
                display: display.clone(),
            },
            ExtendedThinking::Disabled => Self::Disabled,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    disable_all_hooks: bool,
    auto_memory_enabled: bool,
    permissions: Permissions,
}

#[derive(Serialize)]
struct Permissions {
    ask: [&'static str; 1],
}

pub(super) fn metadata(
    prepared: &PreparedRequest,
    env: &BTreeMap<String, String>,
) -> Result<Value, serde_json::Error> {
    let thinking = prepared.thinking.as_ref().map(Thinking::from);
    serde_json::to_value(Extension {
        claude_code: ClaudeCode {
            emit_raw_sdk_messages: true,
            options: Options {
                system_prompt: CustomPrompt::Custom {
                    prompt: &prepared.system_prompt,
                    snapshot: false,
                },
                model: &prepared.model,
                effort: prepared.effort.as_ref(),
                thinking,
                tools: [],
                allowed_tools: [],
                strict_mcp_config: true,
                setting_sources: [],
                settings: Settings {
                    disable_all_hooks: true,
                    auto_memory_enabled: false,
                    // Even unattended JP tools need the adapter callback for
                    // correlation; the JP MCP Host decides whether to prompt.
                    permissions: Permissions {
                        ask: ["mcp__jp__*"],
                    },
                },
                persist_session: !prepared.history.is_empty(),
                env,
                output_format: prepared.schema.as_ref(),
            },
        },
    })
}

/// The adapter's wall-clock ceiling for one tool call, in milliseconds.
///
/// `MCP_TOOL_TIMEOUT` has no "off" spelling, so the ceiling is a number large
/// enough that no prompt outlives it: about 24 days, the most a 32-bit
/// millisecond timer holds.
const NO_TIMEOUT: i32 = i32::MAX;

pub(super) fn environment(
    prepared: &PreparedRequest,
    cache: CachePolicy,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::from([
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("DISABLE_AUTO_COMPACT".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into()),
        ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS".into(), "0".into()),
        ("ENABLE_TOOL_SEARCH".into(), "false".into()),
        ("MAX_MCP_OUTPUT_TOKENS".into(), "100000".into()),
        // A JP tool call stays silent while its approval prompt is open, and
        // the adapter's two per-call timers measure wall-clock time whether or
        // not JP is running: a closed laptop looks exactly like a hung server.
        // Both would otherwise abort the call and hand the model a failure for
        // a question nobody has answered yet.
        //
        // Disabling the idle check rather than sending progress notifications
        // is what survives suspension — a heartbeat only resets the timer if it
        // arrives, and a suspended process sends nothing.
        //
        // Deciding when to stop waiting is JP's job: the interrupt handler
        // cancels a call the user abandons, and the MCP Host holds the reply
        // the service is parked on until then.
        ("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT".into(), "0".into()),
        ("MCP_TOOL_TIMEOUT".into(), NO_TIMEOUT.to_string()),
    ]);
    if let Some(max_tokens) = prepared.max_tokens {
        environment.insert(
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            max_tokens.to_string(),
        );
    }
    if cache == CachePolicy::Off {
        environment.insert("DISABLE_PROMPT_CACHING".into(), "1".into());
    }
    environment
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
