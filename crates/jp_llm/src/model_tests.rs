use jp_config::model::parameters::{ReasoningConfig, ReasoningEffort};

use super::{ModelDetails, ReasoningDetails};

mod custom_reasoning_config {
    use super::*;

    fn model(reasoning: ReasoningDetails) -> ModelDetails {
        let mut details = ModelDetails::empty("openai/test-model".parse().unwrap());
        details.reasoning = Some(reasoning);
        details
    }

    /// A leveled model whose only supported level is `max` resolves `Auto` to
    /// `max` instead of falling through to an unsupported level.
    #[test]
    fn auto_on_max_only_model_selects_max() {
        let details = model(ReasoningDetails::leveled(
            false, false, false, false, false, false, true,
        ));

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Auto))
            .unwrap();

        assert_eq!(config.effort, ReasoningEffort::Max);
    }

    /// `max` is a last resort: any lower supported level wins in the `Auto`
    /// selection.
    #[test]
    fn auto_prefers_lower_levels_over_max() {
        let details = model(ReasoningDetails::leveled(
            false, false, true, false, false, false, true,
        ));

        let config = details
            .custom_reasoning_config(Some(ReasoningConfig::Auto))
            .unwrap();

        assert_eq!(config.effort, ReasoningEffort::Low);
    }
}
