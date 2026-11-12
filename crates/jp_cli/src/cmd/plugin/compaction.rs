//! Compactions a plugin asks the host to add to a conversation.
//!
//! A `compact` request carries typed compactions, each a turn range and the
//! policies to apply to it.
//! Each becomes the rule an inline `--compact SPEC` produces, and is planned
//! and built by the same code as `jp conversation compact`.
//! The request is refused as a whole: nothing is stored unless every compaction
//! in it can be.

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

/// Add `compactions` to the conversation `lock` holds, in order.
///
/// `config` is the conversation's configuration: a summary model named by alias
/// resolves through its aliases, and a summary without a model uses its
/// assistant model.
///
/// # Errors
///
/// Returns why the request was refused, naming the compaction at fault by its
/// 1-based position: one with no policy, an unknown model, a range that selects
/// no turns, a blank summary text, or a summary that would overlap one stored
/// already.
/// A summarizer that fails, or a conversation that can't be written, is an
/// error too.
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

    // Plans every compaction together before the first summary is generated,
    // so an overlap between two of them is refused before a model is paid.
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

/// The rule compaction `number` asks for, as the equivalent inline spec
/// produces it.
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

    // The conversation's model settings with only the id changed, as `jp
    // conversation compact --model` does.
    let mut summary_model = config.assistant.model.clone();
    summary_model.id = ModelIdOrAliasConfig::Id(id);
    if let Some(summary) = rule.summary.as_mut() {
        summary.model = Some(summary_model);
    }

    Ok(rule)
}

/// A turn on the wire as a rule bound: `n` is the 1-based turn `n`, `-n` the
/// `n`th turn from the last.
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

/// The summary settings of a rule; the model is set once the rule is built.
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

/// A stored compaction as the reply reports it, with 1-based turns.
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

/// A refusal as `jp conversation compact` reports it: what went wrong, then
/// what the error adds, such as why a summary would overlap.
///
/// A suggestion is left out: it names the command's flags, which a plugin can't
/// pass.
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
