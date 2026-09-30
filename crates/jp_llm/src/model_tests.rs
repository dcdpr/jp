use jp_config::model::parameters::{CustomReasoningConfig, ReasoningConfig, ReasoningEffort};

use super::{ModelDetails, ReasoningDetails};

mod custom_reasoning_config {
    use super::*;

    fn model(reasoning: ReasoningDetails) -> ModelDetails {
        let mut details = ModelDetails::empty("openai/test-model".parse().unwrap());
        details.reasoning = Some(reasoning);
        details
    }

    /// A model with no reported reasoning support and no user configuration
    /// gets no reasoning config at all, so the request omits the field and the
    /// provider's own default applies.
    #[test]
    fn unknown_support_unconfigured_sends_nothing() {
        let details = ModelDetails::empty("anthropic/whatever".parse().unwrap());
        assert_eq!(details.reasoning, None, "fixture must be unknown");

        assert_eq!(details.custom_reasoning_config(None, None), None);
        assert_eq!(
            details.custom_reasoning_config(Some(ReasoningConfig::Off), None),
            None
        );
    }

    /// `auto` on a model with unknown support enables reasoning without picking
    /// an effort: there is no ladder to choose from, and `auto` asks for the
    /// provider's default rather than one of ours.
    #[test]
    fn unknown_support_auto_defers_effort_to_provider() {
        let details = ModelDetails::empty("anthropic/whatever".parse().unwrap());

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Auto), None)
            .expect("auto enables reasoning");

        assert_eq!(config.effort, ReasoningEffort::Auto);
    }

    /// An explicit effort on a model with unknown support is passed through
    /// rather than dropped.
    #[test]
    fn unknown_support_honours_explicit_effort() {
        let details = ModelDetails::empty("anthropic/whatever".parse().unwrap());

        let config = details
            .custom_reasoning_config(
                Some(ReasoningConfig::Custom(CustomReasoningConfig {
                    effort: ReasoningEffort::High,
                    exclude: false,
                })),
                None,
            )
            .expect("explicit effort enables reasoning");

        assert_eq!(config.effort, ReasoningEffort::High);
    }

    /// A leveled model whose only supported level is `max` resolves `Auto` to
    /// `max` instead of falling through to an unsupported level.
    #[test]
    fn auto_on_max_only_model_selects_max() {
        let details =
            model(ReasoningDetails::leveled(false, false, false, false, false, true).always_on());

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Auto), None)
            .unwrap();

        assert_eq!(config.effort, ReasoningEffort::Max);
    }

    /// `max` is a last resort: any lower supported level wins in the `Auto`
    /// selection.
    #[test]
    fn auto_prefers_lower_levels_over_max() {
        let details =
            model(ReasoningDetails::leveled(false, true, false, false, false, true).always_on());

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Auto), None)
            .unwrap();

        assert_eq!(config.effort, ReasoningEffort::Low);
    }

    /// The effort a model is sent for an explicit `requested` effort, with
    /// absolute efforts resolved against `max_tokens`.
    fn custom_effort_with_limit(
        details: &ModelDetails,
        requested: ReasoningEffort,
        max_tokens: Option<u32>,
    ) -> ReasoningEffort {
        details
            .custom_reasoning_config(
                Some(ReasoningConfig::Custom(CustomReasoningConfig {
                    effort: requested,
                    exclude: false,
                })),
                max_tokens,
            )
            .expect("explicit effort enables reasoning")
            .effort
    }

    /// The effort a model is sent for an explicit `requested` effort.
    fn custom_effort(details: &ModelDetails, requested: ReasoningEffort) -> ReasoningEffort {
        custom_effort_with_limit(details, requested, None)
    }

    /// A level the model supports is sent as asked.
    #[test]
    fn supported_custom_effort_passes_through() {
        let details =
            model(ReasoningDetails::leveled(false, true, true, true, true, true).always_on());

        assert_eq!(
            custom_effort(&details, ReasoningEffort::Medium),
            ReasoningEffort::Medium
        );
    }

    /// GPT-6.1 Sol accepts `low` through `max` and cannot disable reasoning, so
    /// `xlow` and `none` both become `low` rather than `minimal` and `none`,
    /// which it rejects.
    #[test]
    fn efforts_below_the_ladder_clamp_to_its_lowest_level() {
        let details =
            model(ReasoningDetails::leveled(false, true, true, true, true, true).always_on());

        assert_eq!(
            custom_effort(&details, ReasoningEffort::Xlow),
            ReasoningEffort::Low
        );
        assert_eq!(
            custom_effort(&details, ReasoningEffort::None),
            ReasoningEffort::Low
        );
    }

    /// `none` is a valid effort on a model that can disable reasoning, even one
    /// without an `xlow` level.
    #[test]
    fn none_passes_through_when_reasoning_can_be_disabled() {
        let details = model(ReasoningDetails::leveled(
            false, true, true, true, true, false,
        ));

        assert_eq!(
            custom_effort(&details, ReasoningEffort::None),
            ReasoningEffort::None
        );
    }

    /// `max` on a model without it drops to the next level down.
    #[test]
    fn efforts_above_the_ladder_clamp_to_its_highest_level() {
        let details = model(ReasoningDetails::leveled(
            false, true, true, true, true, false,
        ));

        assert_eq!(
            custom_effort(&details, ReasoningEffort::Max),
            ReasoningEffort::XHigh
        );
    }

    /// An effort in a gap between two supported levels rounds up, as on Gemini
    /// 3.1 Pro, which accepts `low` and `high` but not `medium`.
    #[test]
    fn an_effort_between_two_levels_rounds_up() {
        let details =
            model(ReasoningDetails::leveled(false, true, false, true, false, false).always_on());

        assert_eq!(
            custom_effort(&details, ReasoningEffort::Medium),
            ReasoningEffort::High
        );
    }

    /// An absolute token count is converted against the given limit before it
    /// is clamped: 95% of the limit is `max`, which this model lacks.
    #[test]
    fn absolute_efforts_are_resolved_before_clamping() {
        let details = model(ReasoningDetails::leveled(
            false, true, true, true, true, false,
        ));

        assert_eq!(
            custom_effort_with_limit(
                &details,
                ReasoningEffort::Absolute(95_000.into()),
                Some(100_000)
            ),
            ReasoningEffort::XHigh
        );
    }

    /// The limit an absolute effort resolves against is the one the caller
    /// passes, not the model's own output capacity: 12,000 tokens is `low` of
    /// 65,536 but `high` of 16,000.
    #[test]
    fn absolute_efforts_resolve_against_the_given_limit() {
        let mut details =
            model(ReasoningDetails::leveled(false, true, true, true, false, false).always_on());
        details.max_output_tokens = Some(65_536);

        assert_eq!(
            custom_effort_with_limit(
                &details,
                ReasoningEffort::Absolute(12_000.into()),
                Some(16_000)
            ),
            ReasoningEffort::High
        );
    }

    /// `auto` is left for the provider to map to its own default.
    #[test]
    fn auto_custom_effort_passes_through() {
        let details =
            model(ReasoningDetails::leveled(false, true, true, true, true, true).always_on());

        assert_eq!(
            custom_effort(&details, ReasoningEffort::Auto),
            ReasoningEffort::Auto
        );
    }
}
