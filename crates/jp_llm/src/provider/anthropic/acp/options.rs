//! Qualified Claude SDK options assembled from a resolved JP request.

use std::{collections::BTreeMap, time::Duration};

use async_anthropic::types::{Effort, ExtendedThinking};
use jp_config::{assistant::request::CachePolicy, model::id::Name};
use serde::Serialize;
use serde_json::{Map, Value};

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
    output_format: Option<OutputFormat<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CustomPrompt<'a> {
    Custom { prompt: &'a str, snapshot: bool },
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Thinking {
    Enabled { budget_tokens: u32 },
    Adaptive,
    Disabled,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputFormat<'a> {
    JsonSchema { schema: &'a Map<String, Value> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    disable_all_hooks: bool,
    auto_memory_enabled: bool,
}

pub(super) fn metadata(
    prepared: &PreparedRequest,
    env: &BTreeMap<String, String>,
) -> Result<Value, serde_json::Error> {
    let thinking = prepared.thinking.as_ref().map(|thinking| match thinking {
        ExtendedThinking::Enabled { budget_tokens, .. } => Thinking::Enabled {
            budget_tokens: *budget_tokens,
        },
        ExtendedThinking::Adaptive { .. } => Thinking::Adaptive,
        ExtendedThinking::Disabled => Thinking::Disabled,
    });
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
                },
                persist_session: !prepared.history.is_empty(),
                env,
                output_format: prepared
                    .schema
                    .as_ref()
                    .map(|schema| OutputFormat::JsonSchema { schema }),
            },
        },
    })
}

pub(super) fn environment(
    prepared: &PreparedRequest,
    cache: CachePolicy,
) -> BTreeMap<String, String> {
    let ttl = match cache {
        CachePolicy::Long => "1h",
        CachePolicy::Custom(duration) if duration >= Duration::from_mins(30) => "1h",
        _ => "5m",
    };
    BTreeMap::from([
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("DISABLE_AUTO_COMPACT".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into()),
        ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS".into(), "0".into()),
        (
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            prepared.max_tokens.to_string(),
        ),
        ("ENABLE_TOOL_SEARCH".into(), "false".into()),
        (
            "DISABLE_PROMPT_CACHING".into(),
            if cache == CachePolicy::Off { "1" } else { "0" }.into(),
        ),
        ("CLAUDE_CODE_PROMPT_CACHE_TTL".into(), ttl.into()),
        ("MAX_MCP_OUTPUT_TOKENS".into(), "100000".into()),
    ])
}
