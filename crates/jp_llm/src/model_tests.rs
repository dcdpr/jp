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

        assert_eq!(details.custom_reasoning_config(None), None);
        assert_eq!(
            details.custom_reasoning_config(Some(ReasoningConfig::Off)),
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
            .custom_reasoning_config(Some(ReasoningConfig::Auto))
            .expect("auto enables reasoning");

        assert_eq!(config.effort, ReasoningEffort::Auto);
    }

    /// An explicit effort on a model with unknown support is passed through
    /// rather than dropped.
    #[test]
    fn unknown_support_honours_explicit_effort() {
        let details = ModelDetails::empty("anthropic/whatever".parse().unwrap());

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Custom(CustomReasoningConfig {
                effort: ReasoningEffort::High,
                exclude: false,
            })))
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
            .custom_reasoning_config(Some(ReasoningConfig::Auto))
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
            .custom_reasoning_config(Some(ReasoningConfig::Auto))
            .unwrap();

        assert_eq!(config.effort, ReasoningEffort::Low);
    }
}
