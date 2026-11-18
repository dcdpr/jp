// qual:allow(srp, file_length=1100) reason: "one static model table; split in T-1hb8d4k"
//! The OpenAI model catalog.
//!
//! [`map_model`] turns a model id into its [`ModelDetails`], looked up in
//! [`MODEL_OVERRIDES`] under the canonical id or an alias.

use std::sync::LazyLock;

use chrono::NaiveDate;
use jp_config::model::id::ModelIdConfig;
use tracing::warn;

use super::{
    EXPLICIT_PROMPT_CACHING, ModelResponse, PERSISTED_REASONING, PROVIDER, REASONING_PRO_MODE,
    STREAMING_UNSUPPORTED, TEMP_REQUIRES_NO_REASONING,
};
use crate::{
    error::Result,
    model::{
        ModelDeprecation, ModelDetails, ReasoningDetails,
        catalog::{Catalog, Entry},
    },
};

/// Map an OpenAI model id onto its capabilities.
///
/// An id the catalog does not know maps to empty details, with a warning.
pub(super) fn map_model(model: ModelResponse) -> Result<ModelDetails> {
    if let Some(details) = model_overrides(&model.id) {
        // Reported under the id the caller named, which may be an alias.
        let mut details = details.clone();
        details.id = (PROVIDER, model.id).try_into()?;
        return Ok(details);
    }

    warn!(model = model.id.as_str(), "Missing model details.");
    Ok(ModelDetails::empty((PROVIDER, model.id).try_into()?))
}

/// Whether the catalog marks `model` as served only through the API.
///
/// A model the catalog does not know is not API-only: the plan's model set
/// changes without a JP release, and refusing it here would hide a model the
/// subscription may well serve.
pub(super) fn is_api_only(model: &str) -> bool {
    model_overrides(model).is_some_and(|details| details.subscription == Some(false))
}

/// The canonical ids of the models a `ChatGPT` subscription serves, in catalog
/// order.
pub(super) fn subscription_models() -> impl Iterator<Item = &'static str> {
    MODEL_OVERRIDES
        .values()
        .filter(|details| details.served_by_subscription())
        .map(|details| details.id.name.as_ref())
}

/// Look up the details for `id`, under its canonical id or an alias.
///
/// `None` for a model absent from [`MODEL_OVERRIDES`].
fn model_overrides(id: &str) -> Option<&'static ModelDetails> {
    MODEL_OVERRIDES.get(id)
}

/// The OpenAI model details the API does not report, which is all of them.
///
/// This table is authoritative rather than a fallback: `GET /v1/models/{id}`
/// returns only `{id, object, created, owned_by}`, reporting neither context
/// windows nor reasoning effort levels.
/// Unlike the Anthropic, OpenRouter, and Cerebras providers, there is nothing
/// to derive from, so every value here is maintained by hand against OpenAI's
/// published model documentation.
pub(super) static MODEL_OVERRIDES: LazyLock<Catalog<ModelDetails>> = LazyLock::new(|| {
    let date = |year, month, day| NaiveDate::from_ymd_opt(year, month, day).unwrap();
    let id = |name: &str| ModelIdConfig::try_from((PROVIDER, name)).unwrap();

    Catalog::new(vec![
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6.2-astra"),
                display_name: Some("GPT-6.2 Astra".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort rejects `none`, and the migration guide maps
                // `minimal` to `low`, so the lowest level stands in for a
                // disable.
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, true).always_on(),
                ),
                knowledge_cutoff: Some(date(2026, 5, 18)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6.2-sol"),
                display_name: Some("GPT-6.2 Sol".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort rejects `none`, and the migration guide maps
                // `minimal` to `low`, so the lowest level stands in for a
                // disable.
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, true).always_on(),
                ),
                knowledge_cutoff: Some(date(2026, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6-astra"),
                display_name: Some("GPT-6 Astra".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: low, medium, high, xhigh, max. There
                // is no `none`, so reasoning cannot be turned off; the lowest
                // level stands in for a disable.
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, true).always_on(),
                ),
                knowledge_cutoff: Some(date(2026, 4, 30)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                // Reasoning is always active, so TEMP_REQUIRES_NO_REASONING drops
                // temperature and top_p on every request, which is what this model
                // wants: it rejects both outright.
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6.1-sol"),
                display_name: Some("GPT-6.1 Sol".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: low, medium, high, xhigh, max. Neither
                // `none` nor `minimal` is accepted, so reasoning cannot be turned
                // off; the lowest level stands in for a disable.
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, true).always_on(),
                ),
                knowledge_cutoff: Some(date(2026, 4, 30)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6.1-terra"),
                display_name: Some("GPT-6.1 Terra".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none, low, medium, high, xhigh, max.
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6-sol"),
                display_name: Some("GPT-6 Sol".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none, low, medium, high, xhigh, max.
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 4, 20)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-6-luna"),
                display_name: Some("GPT-6 Luna".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none, low, medium, high, xhigh, max.
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 5, 18)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &["gpt-5.6"],
            value: ModelDetails {
                id: id("gpt-5.6-sol"),
                display_name: Some("GPT-5.6 Sol".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none, low, medium, high, xhigh, max.
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 2, 16)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.6-terra"),
                display_name: Some("GPT-5.6 Terra".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 2, 16)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.6-luna"),
                display_name: Some("GPT-5.6 Luna".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, true,
                )),
                knowledge_cutoff: Some(date(2026, 2, 16)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![
                    TEMP_REQUIRES_NO_REASONING,
                    REASONING_PRO_MODE,
                    PERSISTED_REASONING,
                    EXPLICIT_PROMPT_CACHING,
                ],
            },
        },
        Entry {
            aliases: &["gpt-5.5-2026-04-23"],
            value: ModelDetails {
                id: id("gpt-5.5"),
                display_name: Some("GPT-5.5".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 12, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.5-pro-2026-04-23"],
            value: ModelDetails {
                id: id("gpt-5.5-pro"),
                display_name: Some("GPT-5.5 pro".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, false, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 12, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING, STREAMING_UNSUPPORTED],
            },
        },
        Entry {
            aliases: &["gpt-5.4-2026-03-05"],
            value: ModelDetails {
                id: id("gpt-5.4"),
                display_name: Some("GPT-5.4".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.4-pro-2026-03-05"],
            value: ModelDetails {
                id: id("gpt-5.4-pro"),
                display_name: Some("GPT-5.4 pro".to_owned()),
                context_window: Some(1_050_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, false, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.4-mini-2026-03-17"],
            value: ModelDetails {
                id: id("gpt-5.4-mini"),
                display_name: Some("GPT-5.4 mini".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.4-nano-2026-03-17"],
            value: ModelDetails {
                id: id("gpt-5.4-nano"),
                display_name: Some("GPT-5.4 nano".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        // Codex's ultra-fast tier, served only through a subscription.
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.3-codex-spark"),
                display_name: Some("GPT-5.3 Codex Spark".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, false, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(true),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.3-codex"),
                display_name: Some("GPT-5.3 Codex".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.3-chat-latest"),
                display_name: Some("GPT-5.3 Chat".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 8, 10)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.2-codex"),
                display_name: Some("GPT-5.2 Codex".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: low, medium, high, xhigh (no none)
                reasoning: Some(
                    ReasoningDetails::leveled(false, true, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.2-pro-2025-12-11"],
            value: ModelDetails {
                id: id("gpt-5.2-pro"),
                display_name: Some("GPT-5.2 pro".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, false, true, true, true, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.2-2025-12-11"],
            value: ModelDetails {
                id: id("gpt-5.2"),
                display_name: Some("GPT-5.2".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none (default), low, medium, high, xhigh
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.2-chat-latest"),
                display_name: Some("GPT-5.2 Chat".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, true, false,
                )),
                knowledge_cutoff: Some(date(2025, 8, 31)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 8, 10)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.1-codex-max"),
                display_name: Some("GPT-5.1-Codex-Max".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.1-codex"),
                display_name: Some("GPT-5.1 Codex".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.1-codex-mini"),
                display_name: Some("GPT-5.1 Codex mini".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.4-mini",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5.1-2025-11-13"],
            value: ModelDetails {
                id: id("gpt-5.1"),
                display_name: Some("GPT-5.1".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: none (default), low, medium, high
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, false, false,
                )),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5.1-chat-latest"),
                display_name: Some("GPT-5.1 Chat".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(ReasoningDetails::leveled(
                    false, true, true, true, false, false,
                )),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5-codex"),
                display_name: Some("GPT-5-Codex".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5"),
                display_name: Some("GPT-5".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: minimal, low, medium, high
                reasoning: Some(
                    ReasoningDetails::leveled(true, true, true, true, false, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                // Deprecated without an announced retirement date; only the
                // 2025-08-07 snapshot has a scheduled shutdown (2026-12-11).
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    None,
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5-2025-08-07"),
                display_name: Some("GPT-5".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                // Reasoning.effort supports: minimal, low, medium, high
                reasoning: Some(
                    ReasoningDetails::leveled(true, true, true, true, false, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5-pro-2025-10-06"],
            value: ModelDetails {
                id: id("gpt-5-pro"),
                display_name: Some("GPT-5 pro".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(
                    ReasoningDetails::leveled(false, false, false, true, false, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5-pro",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-5-chat-latest"),
                display_name: Some("GPT-5 Chat".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(
                    ReasoningDetails::leveled(true, true, true, true, false, false).always_on(),
                ),
                knowledge_cutoff: Some(date(2024, 9, 30)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5-mini-2025-08-07"],
            value: ModelDetails {
                id: id("gpt-5-mini"),
                display_name: Some("GPT-5 mini".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 5, 31)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.4-mini",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![TEMP_REQUIRES_NO_REASONING],
            },
        },
        Entry {
            aliases: &["gpt-5-nano-2025-08-07"],
            value: ModelDetails {
                id: id("gpt-5-nano"),
                display_name: Some("GPT-5 nano".to_owned()),
                context_window: Some(400_000),
                max_output_tokens: Some(128_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 5, 31)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.4-nano",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o4-mini-2025-04-16"],
            value: ModelDetails {
                id: id("o4-mini"),
                display_name: Some("o4-mini".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.4-mini",
                    Some(date(2026, 10, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o3-mini-2025-01-31"],
            value: ModelDetails {
                id: id("o3-mini"),
                display_name: Some("o3-mini".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2023, 10, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 10, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o3-2025-04-16"],
            value: ModelDetails {
                id: id("o3"),
                display_name: Some("o3".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o3-pro-2025-06-10"],
            value: ModelDetails {
                id: id("o3-pro"),
                display_name: Some("o3-pro".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5-pro",
                    Some(date(2026, 12, 11)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o1-2024-12-17"],
            value: ModelDetails {
                id: id("o1"),
                display_name: Some("o1".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2023, 10, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    Some(date(2026, 10, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o1-pro-2025-03-19"],
            value: ModelDetails {
                id: id("o1-pro"),
                display_name: Some("o1-pro".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2023, 10, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5-pro",
                    Some(date(2026, 10, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["gpt-4.1-2025-04-14"],
            value: ModelDetails {
                id: id("gpt-4.1"),
                display_name: Some("GPT-4.1".to_owned()),
                context_window: Some(1_047_576),
                max_output_tokens: Some(32_768),
                reasoning: Some(ReasoningDetails::unsupported()),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["gpt-4o-2024-08-06", "gpt-4o-2024-11-20"],
            value: ModelDetails {
                id: id("gpt-4o"),
                display_name: Some("GPT-4o".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(ReasoningDetails::unsupported()),
                knowledge_cutoff: Some(date(2023, 10, 1)),
                // Deprecated without an announced retirement date; only the
                // 2024-05-13 snapshot has a scheduled shutdown (2026-10-23).
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5",
                    None,
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["gpt-4.1-nano-2025-04-14"],
            value: ModelDetails {
                id: id("gpt-4.1-nano"),
                display_name: Some("GPT-4.1 nano".to_owned()),
                context_window: Some(1_047_576),
                max_output_tokens: Some(32_768),
                reasoning: Some(ReasoningDetails::unsupported()),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.4-nano",
                    Some(date(2026, 10, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["gpt-4o-mini-2024-07-18"],
            value: ModelDetails {
                id: id("gpt-4o-mini"),
                display_name: Some("GPT-4o mini".to_owned()),
                context_window: Some(128_000),
                max_output_tokens: Some(16_384),
                reasoning: Some(ReasoningDetails::unsupported()),
                knowledge_cutoff: Some(date(2023, 10, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["gpt-4.1-mini-2025-04-14"],
            value: ModelDetails {
                id: id("gpt-4.1-mini"),
                display_name: Some("GPT-4.1 mini".to_owned()),
                context_window: Some(1_047_576),
                max_output_tokens: Some(32_768),
                reasoning: Some(ReasoningDetails::unsupported()),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-oss-120b"),
                display_name: Some("gpt-oss-120b".to_owned()),
                context_window: Some(131_072),
                max_output_tokens: Some(131_072),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &[],
            value: ModelDetails {
                id: id("gpt-oss-20b"),
                display_name: Some("gpt-oss-20b".to_owned()),
                context_window: Some(131_072),
                max_output_tokens: Some(131_072),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::Active),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o3-deep-research-2025-06-26"],
            value: ModelDetails {
                id: id("o3-deep-research"),
                display_name: Some("o3-deep-research".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5-pro",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
        Entry {
            aliases: &["o4-mini-deep-research-2025-06-26"],
            value: ModelDetails {
                id: id("o4-mini-deep-research"),
                display_name: Some("o4-mini-deep-research".to_owned()),
                context_window: Some(200_000),
                max_output_tokens: Some(100_000),
                reasoning: Some(ReasoningDetails::budgetted(0, None)),
                knowledge_cutoff: Some(date(2024, 6, 1)),
                deprecated: Some(ModelDeprecation::deprecated(
                    &"recommended replacement: gpt-5.5-pro",
                    Some(date(2026, 7, 23)),
                )),
                structured_output: None,
                prefill: None,
                subscription: Some(false),
                features: vec![],
            },
        },
    ])
});
