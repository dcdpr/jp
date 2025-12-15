use jp_config::model::{
    id::ModelIdConfig,
    parameters::{CustomReasoningConfig, ReasoningConfig, ReasoningEffort},
};
use time::Date;
use tracing::warn;

/// Details about a model for a given provider, as specified by the provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDetails {
    /// The id of the model.
    pub id: ModelIdConfig,

    /// The display name of the model, if known.
    pub display_name: Option<String>,

    /// The context window size in tokens, if known.
    pub context_window: Option<u32>,

    /// The maximum output tokens, if known.
    pub max_output_tokens: Option<u32>,

    /// Whether the model supports reasoning, if unknown, this value is left to
    /// `None`.
    pub reasoning: Option<ReasoningDetails>,

    /// The knowledge cutoff date, if known.
    pub knowledge_cutoff: Option<Date>,

    /// Deprecation status of the model, if known.
    pub deprecated: Option<ModelDeprecation>,

    /// Provider-specific features.
    pub features: Vec<&'static str>,
}

impl ModelDetails {
    #[must_use]
    pub fn empty(id: ModelIdConfig) -> Self {
        Self {
            id,
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            reasoning: None,
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        }
    }

    #[must_use]
    pub fn custom_reasoning_config(
        &self,
        config: Option<ReasoningConfig>,
    ) -> Option<CustomReasoningConfig> {
        match self.reasoning {
            // Unknown support
            None => match config {
                // Unconfigured or off, so disabled.
                None | Some(ReasoningConfig::Off) => None,

                // Auto configured, so use medium effort.
                Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },

            // Unsupported
            Some(ReasoningDetails::Unsupported) => match config {
                // Unconfigured, auto or off, so disabled.
                None | Some(ReasoningConfig::Auto | ReasoningConfig::Off) => None,

                // Custom configuration, invalid, so warn + disabled.
                Some(ReasoningConfig::Custom(config)) => {
                    warn!(
                        id = %self.id,
                        ?config,
                        "Model does not support reasoning, but the configuration explicitly \
                        enabled it. Reasoning will be disabled to avoid failed requests."
                    );

                    None
                }
            },

            // Budgetted
            Some(ReasoningDetails::Budgetted { .. }) => match config {
                // Off, so disabled.
                Some(ReasoningConfig::Off) => None,

                // Unconfigured, or auto, so medium effort.
                None | Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },

            // Leveled
            Some(ReasoningDetails::Leveled {
                low: _,
                medium,
                high,
                xhigh,
            }) => match config {
                // Off, so disabled.
                Some(ReasoningConfig::Off) => None,

                // Auto configured, so use medium effort if the model supports
                // it, otherwise high or low.
                None | Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: if medium {
                        ReasoningEffort::Medium
                    } else if high {
                        ReasoningEffort::High
                    } else if xhigh {
                        ReasoningEffort::XHigh
                    } else {
                        ReasoningEffort::Low
                    },
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },
        }
    }
}

/// The deprecation status of a model.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelDeprecation {
    /// The model is active and available for use.
    #[default]
    Active,

    /// The model is deprecated and will be removed at some point in the future.
    Deprecated {
        /// Any details about the deprecation.
        ///
        /// This could include a link to the deprecation notice, a reason for
        /// deprecation, or recommended replacements.
        note: String,

        /// The date on which the model will be retired, if known.
        retire_at: Option<Date>,
    },
}

impl ModelDeprecation {
    pub fn deprecated(note: &impl ToString, retire_at: Option<Date>) -> Self {
        Self::Deprecated {
            note: note.to_string(),
            retire_at,
        }
    }
}

/// Details about the reasoning capabilities of a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReasoningDetails {
    /// Reasoning is not supported.
    Unsupported,

    /// Budgetted reasoning support.
    ///
    /// Most models allow specifying the minimum and maximum number of tokens
    /// that the model can use to "reason".
    Budgetted {
        /// The minimum number of reasoning tokens required to generate a
        /// response. Usually zero, but can be non-zero for certain models.
        min_tokens: u32,

        /// The maximum number of reasoning tokens that can be generated.
        max_tokens: Option<u32>,
    },

    /// Level-based reasoning support.
    ///
    /// Some models, such as Google's Gemini 3, do not support token-based
    /// reasoning configuration, but instead offer specific "efforts" of
    /// reasoning, such as low/medium/high effort.
    Leveled {
        /// Whether the model supports low effort reasoning.
        low: bool,

        /// Whether the model supports medium effort reasoning.
        medium: bool,

        /// Whether the model supports high effort reasoning.
        high: bool,

        /// Whether the model supports extremely high effort reasoning.
        xhigh: bool,
    },
}

impl ReasoningDetails {
    #[must_use]
    pub fn budgetted(min_tokens: u32, max_tokens: Option<u32>) -> Self {
        Self::Budgetted {
            min_tokens,
            max_tokens,
        }
    }

    #[must_use]
    #[expect(clippy::fn_params_excessive_bools)]
    pub fn leveled(low: bool, medium: bool, high: bool, xhigh: bool) -> Self {
        Self::Leveled {
            low,
            medium,
            high,
            xhigh,
        }
    }

    #[must_use]
    pub fn unsupported() -> Self {
        Self::Unsupported
    }

    #[must_use]
    pub fn min_tokens(&self) -> u32 {
        match self {
            Self::Budgetted { min_tokens, .. } => *min_tokens,
            Self::Leveled { .. } | Self::Unsupported => 0,
        }
    }

    #[must_use]
    pub fn max_tokens(&self) -> Option<u32> {
        match self {
            Self::Budgetted { max_tokens, .. } => *max_tokens,
            Self::Leveled { .. } | Self::Unsupported => None,
        }
    }

    #[must_use]
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported)
    }

    #[must_use]
    pub fn is_budgetted(&self) -> bool {
        matches!(self, Self::Budgetted { .. })
    }

    #[must_use]
    pub fn is_leveled(&self) -> bool {
        matches!(self, Self::Leveled { .. })
    }
}
