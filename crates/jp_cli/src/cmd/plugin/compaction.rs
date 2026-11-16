//! Handler for plugin `compact` requests.
//!
//! Converts each [`Compaction`] in a `compact` request into a
//! [`CompactionRuleConfig`] and runs it through the same planning and execution
//! pipeline as `jp conversation compact`.
//! Validation is atomic: no compactions are persisted unless every entry in the
//! request succeeds.

use std::{num::NonZeroI64, slice};

use jp_config::{
    AppConfig,
    conversation::compaction::{
        CompactionConfig, CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig,
        ReasoningMode, RuleBound, ToolCallsMode as RuleToolCallsMode,
    },
    model::id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig},
    types::{byte_size::ByteSize, policy_spec::PolicySpec},
};
use jp_conversation::{Compaction as StoredEvent, SummarySource as StoredSource, ToolCallPolicy};
use jp_llm::event::NoticeSink;
use jp_plugin::message::{
    Compaction, GenerateSummary, StoredCompaction, SummaryCompaction, SummarySource, ToolCallsMode,
};
use jp_workspace::ConversationLock;
use tracing::warn;

use crate::cmd::{
    self,
    conversation::compact::{apply_compactions, build_compaction_events, plan_compactions},
    turn_selection::TurnSelection,
};

/// Apply `compactions` in order to the locked conversation.
///
/// Resolves model aliases and the default summarizer model against `config`.
///
/// # Errors
///
/// Returns an error if the request is empty, if any compaction (identified by
/// its 1-based index) has no policy, names an unknown model, selects no turns,
/// provides blank summary text, or overlaps an existing summary, or if summary
/// generation or persistence fails.
pub(crate) async fn compact(
    compactions: &[Compaction],
    config: &AppConfig,
    lock: ConversationLock,
) -> Result<Vec<StoredCompaction>, String> {
    if compactions.is_empty() {
        return Err("the request names no compactions".to_owned());
    }

    let rules = compactions
        .iter()
        .enumerate()
        .map(|(index, compaction)| rule(index + 1, compaction, config))
        .collect::<Result<Vec<_>, _>>()?;

    let events = lock.events().clone();
    let selection = TurnSelection::default();
    for (index, (compaction, rule)) in compactions.iter().zip(&rules).enumerate() {
        let planned =
            plan_compactions(&events, slice::from_ref(rule), &selection).map_err(|error| {
                format!(
                    "compaction {}: {}",
                    index + 1,
                    describe(&cmd::Error::from(error))
                )
            })?;
        if planned.is_empty() {
            return Err(format!(
                "compaction {} (turns {}..{}) selects none of the conversation's {} turns",
                index + 1,
                compaction.from,
                compaction.to,
                events.turn_count()
            ));
        }
    }

    // Plan all compactions together before generating summaries so overlaps
    // between rules are caught before calling a model.
    let notices = NoticeSink::new(|notice| warn!(%notice, "Compaction notice."));
    let built = build_compaction_events(&events, config, &rules, &selection, None, &notices)
        .await
        .map_err(|error| describe(&cmd::Error::from(error)))?;

    let stored = built.iter().map(stored).collect();
    let mut conversation = lock.into_mut();
    apply_compactions(&conversation, built);
    conversation
        .flush()
        .map_err(|error| format!("failed to write the conversation: {error}"))?;

    Ok(stored)
}

/// Convert the 1-based compaction `number` into a [`CompactionRuleConfig`].
fn rule(
    number: usize,
    compaction: &Compaction,
    config: &AppConfig,
) -> Result<CompactionRuleConfig, String> {
    if compaction.reasoning.is_none()
        && compaction.tool_calls.is_none()
        && compaction.summary.is_none()
    {
        return Err(format!(
            "compaction {number} has nothing to do: set `reasoning`, `tool_calls`, or `summary`"
        ));
    }

    let partial = PartialCompactionRuleConfig {
        keep_first: Some(bound(compaction.from)),
        keep_last: Some(bound(compaction.to)),
        reasoning: compaction.reasoning.as_ref().map(|reasoning| PolicySpec {
            policy: ReasoningMode::Strip,
            over: reasoning.over_bytes.map(ByteSize::from_bytes),
        }),
        tool_calls: compaction.tool_calls.as_ref().map(|tool_calls| PolicySpec {
            policy: rule_mode(tool_calls.mode),
            over: tool_calls.over_bytes.map(ByteSize::from_bytes),
        }),
        summary: compaction.summary.as_ref().map(summary),
    };

    let mut rule = CompactionConfig::finalize_rules(vec![partial])
        .map_err(|error| format!("compaction {number}: {error}"))?
        .remove(0);

    let Some(SummaryCompaction::Generate(GenerateSummary {
        model: Some(model), ..
    })) = &compaction.summary
    else {
        return Ok(rule);
    };
    let id = PartialModelIdOrAliasConfig::from(model.as_str())
        .resolve(&config.providers.llm.aliases)
        .map_err(|error| format!("compaction {number}: unknown model `{model}`: {error}"))?;

    // Keep the conversation's model parameters and override only the model ID,
    // matching `jp conversation compact --model`.
    let mut summary_model = config.assistant.model.clone();
    summary_model.id = ModelIdOrAliasConfig::Id(id);
    if let Some(summary) = rule.summary.as_mut() {
        summary.model = Some(summary_model);
    }

    Ok(rule)
}

/// Convert a wire turn index into a [`RuleBound`] (`n` is 1-based from the
/// start; `-n` is `n`th from the end).
fn bound(turn: NonZeroI64) -> RuleBound {
    let count = usize::try_from(turn.get().unsigned_abs()).unwrap_or(usize::MAX);

    if turn.get() > 0 {
        RuleBound::Absolute(count)
    } else {
        RuleBound::FromEnd(count - 1)
    }
}

fn rule_mode(mode: ToolCallsMode) -> RuleToolCallsMode {
    match mode {
        ToolCallsMode::Strip => RuleToolCallsMode::Strip,
        ToolCallsMode::StripRequests => RuleToolCallsMode::StripRequests,
        ToolCallsMode::StripResponses => RuleToolCallsMode::StripResponses,
        ToolCallsMode::Omit => RuleToolCallsMode::Omit,
    }
}

/// Build partial summary settings; `rule` resolves the model after finalizing.
fn summary(summary: &SummaryCompaction) -> PartialSummaryConfig {
    match summary {
        SummaryCompaction::Text(text) => PartialSummaryConfig {
            text: Some(text.clone()),
            ..PartialSummaryConfig::default()
        },
        SummaryCompaction::Generate(generate) => PartialSummaryConfig {
            instructions: generate.instructions.clone(),
            context: generate.context.clone(),
            ..PartialSummaryConfig::default()
        },
    }
}

/// Convert a stored compaction event to its 1-based wire representation.
fn stored(compaction: &StoredEvent) -> StoredCompaction {
    let turn = |index: usize| u64::try_from(index + 1).unwrap_or(u64::MAX);

    StoredCompaction {
        from: turn(compaction.from_turn),
        to: turn(compaction.to_turn),
        reasoning: compaction.reasoning.is_some(),
        tool_calls: compaction
            .tool_calls
            .as_ref()
            .and_then(|spec| wire_mode(&spec.policy)),
        summary: compaction
            .summary
            .as_ref()
            .map(|summary| match summary.source {
                StoredSource::Authored => SummarySource::Text,
                StoredSource::Generated => SummarySource::Generated,
            }),
    }
}

fn wire_mode(policy: &ToolCallPolicy) -> Option<ToolCallsMode> {
    match policy {
        ToolCallPolicy::Strip {
            request: true,
            response: true,
        } => Some(ToolCallsMode::Strip),
        ToolCallPolicy::Strip {
            request: true,
            response: false,
        } => Some(ToolCallsMode::StripRequests),
        ToolCallPolicy::Strip {
            request: false,
            response: true,
        } => Some(ToolCallsMode::StripResponses),
        ToolCallPolicy::Strip {
            request: false,
            response: false,
        } => None,
        ToolCallPolicy::Omit => Some(ToolCallsMode::Omit),
    }
}

/// Format a compaction error with its metadata details, omitting CLI flag
/// suggestions (`suggestion`) that do not apply to plugin requests.
fn describe(error: &cmd::Error) -> String {
    let Some(message) = &error.message else {
        return error.to_string();
    };

    let details: Vec<String> = error
        .metadata
        .iter()
        .filter(|(key, _)| key != "suggestion")
        .map(|(_, value)| match value.as_str() {
            Some(value) => value.to_owned(),
            None => value.to_string(),
        })
        .collect();
    if details.is_empty() {
        return message.clone();
    }

    format!("{message}: {}", details.join(" "))
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
