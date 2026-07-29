use chrono::NaiveDate;
use jp_config::model::{
    id::ModelIdConfig,
    parameters::{CustomReasoningConfig, ReasoningConfig, ReasoningEffort},
};
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
    pub knowledge_cutoff: Option<NaiveDate>,

    /// Deprecation status of the model, if known.
    pub deprecated: Option<ModelDeprecation>,

    /// Whether the model supports structured output (JSON schema responses).
    ///
    /// `None` means unknown (e.g. custom deployments).
    /// Providers should set this to `Some(true)` or `Some(false)` for known
    /// models.
    pub structured_output: Option<bool>,

    /// Whether the model accepts a trailing assistant message as a prefill to
    /// continue from.
    ///
    /// Models that reject prefill require the conversation to end with a user
    /// message, so callers must inject a synthetic continuation instead.
    ///
    /// `None` means the provider reports nothing either way.
    pub prefill: Option<bool>,

    /// Provider-specific features.
    ///
    /// Reserved for capabilities read only by the provider that declares them.
    /// A capability that shared code reads belongs in a typed field instead.
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
            structured_output: None,
            prefill: None,
            features: vec![],
        }
    }

    /// Returns `true` if the model is known to support structured output.
    #[must_use]
    pub fn supports_structured_output(&self) -> bool {
        self.structured_output.unwrap_or(false)
    }

    /// Returns `true` if the model is known to support assistant prefill.
    ///
    /// Unknown support reads as `false`: injecting a synthetic continuation for
    /// a model that would have accepted a prefill costs a few tokens, whereas
    /// prefilling one that rejects it is a request error.
    #[must_use]
    pub fn supports_prefill(&self) -> bool {
        self.prefill.unwrap_or(false)
    }

    /// Returns `true` if the model is *known* to support disabling reasoning.
    ///
    /// Models that always run with adaptive thinking, and reject an explicit
    /// `thinking: disabled`, return `false`.
    /// For those, callers must omit the thinking field rather than disabling
    /// it.
    ///
    /// Unknown support also returns `false`, so this answers "may I assume a
    /// disable is safe?" rather than "is a disable worth attempting?".
    /// Callers deciding whether to honour an explicit `reasoning = off` should
    /// treat unknown support as worth attempting and let the endpoint reject
    /// it; this accessor is for the conservative question, such as whether a
    /// forced tool call has to be soft-forced.
    #[must_use]
    pub fn supports_disabling_thinking(&self) -> bool {
        self.reasoning.is_some_and(|r| r.can_disable())
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id.name)
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

                // Auto configured, so enable reasoning without dictating an
                // effort. Support is unknown, so there is no ladder to pick a
                // level from, and `auto` asks for the provider's own default
                // rather than a level of our choosing.
                Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::Auto,
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
            Some(ReasoningDetails::Supported {
                mode: ReasoningMode::Budgetted { .. },
                ..
            }) => match config {
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
            Some(ReasoningDetails::Supported {
                mode:
                    ReasoningMode::Leveled {
                        xlow: _,
                        low,
                        medium,
                        high,
                        xhigh,
                        max,
                    },
                ..
            }) => match config {
                // Off, so disabled.
                Some(ReasoningConfig::Off) => None,

                // Auto configured, so use medium effort if the model supports
                // it, otherwise the nearest supported level.
                None | Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: if medium {
                        ReasoningEffort::Medium
                    } else if high {
                        ReasoningEffort::High
                    } else if xhigh {
                        ReasoningEffort::XHigh
                    } else if low {
                        ReasoningEffort::Low
                    } else if max {
                        ReasoningEffort::Max
                    } else {
                        ReasoningEffort::Xlow
                    },
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },

            // Adaptive
            Some(ReasoningDetails::Supported {
                mode: ReasoningMode::Adaptive { .. },
                ..
            }) => match config {
                // Off, so disabled.
                Some(ReasoningConfig::Off) => None,

                // Unconfigured or auto, so use high effort (the API default).
                None | Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::High,
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
        retire_at: Option<NaiveDate>,
    },
}

impl ModelDeprecation {
    pub fn deprecated(note: &impl ToString, retire_at: Option<NaiveDate>) -> Self {
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

    /// Reasoning is supported, in the given mode.
    Supported {
        /// How this model expresses reasoning effort.
        mode: ReasoningMode,

        /// Whether reasoning can be turned off entirely.
        ///
        /// Models that always reason reject an explicit "disable" request, so
        /// callers must omit the field rather than disabling it.
        ///
        /// For [`ReasoningMode::Leveled`] this additionally asserts that the
        /// provider accepts an explicit "none" effort *value* on the wire,
        /// which is what [`lowest_effort`] encodes by returning
        /// [`ReasoningEffort::None`].
        /// A leveled model that disables by omitting the field instead must not
        /// set this, or it will be sent a level it never announced.
        ///
        /// [`lowest_effort`]: ReasoningDetails::lowest_effort
        can_disable: bool,
    },
}

/// How a model expresses reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReasoningMode {
    /// Budgetted reasoning support.
    ///
    /// Most models allow specifying the minimum and maximum number of tokens
    /// that the model can use to "reason".
    Budgetted {
        /// The minimum number of reasoning tokens required to generate a
        /// response.
        /// Usually zero, but can be non-zero for certain models.
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
        /// Whether the model supports extremely low effort reasoning.
        xlow: bool,

        /// Whether the model supports low effort reasoning.
        low: bool,

        /// Whether the model supports medium effort reasoning.
        medium: bool,

        /// Whether the model supports high effort reasoning.
        high: bool,

        /// Whether the model supports extremely high effort reasoning.
        xhigh: bool,

        /// Whether the model supports maximum effort reasoning, with no
        /// constraints on token spending.
        max: bool,
    },

    /// Adaptive reasoning support.
    ///
    /// The model dynamically decides when and how much to think based on task
    /// complexity.
    /// Uses effort levels (low/medium/high/xhigh/max) instead of token budgets.
    ///
    /// Currently only supported by Claude Opus 4.6+.
    Adaptive {
        /// Whether the model supports `xhigh` (extra high) effort level.
        xhigh: bool,

        /// Whether the model supports `max` effort level.
        max: bool,
    },
}

impl ReasoningDetails {
    /// Reasoning expressed as a token budget.
    ///
    /// Reasoning is disableable; chain [`always_on`] for models that cannot
    /// turn it off.
    ///
    /// [`always_on`]: Self::always_on
    #[must_use]
    pub fn budgetted(min_tokens: u32, max_tokens: Option<u32>) -> Self {
        Self::supported(ReasoningMode::Budgetted {
            min_tokens,
            max_tokens,
        })
    }

    /// Reasoning expressed as discrete effort levels.
    ///
    /// Reasoning is disableable; chain [`always_on`] for models that cannot
    /// turn it off.
    ///
    /// [`always_on`]: Self::always_on
    #[must_use]
    #[expect(clippy::fn_params_excessive_bools)]
    pub fn leveled(
        xlow: bool,
        low: bool,
        medium: bool,
        high: bool,
        xhigh: bool,
        max: bool,
    ) -> Self {
        Self::supported(ReasoningMode::Leveled {
            xlow,
            low,
            medium,
            high,
            xhigh,
            max,
        })
    }

    /// Reasoning the model schedules itself, steered by effort level.
    ///
    /// Reasoning is disableable; chain [`always_on`] for models that cannot
    /// turn it off.
    ///
    /// [`always_on`]: Self::always_on
    #[must_use]
    pub fn adaptive(xhigh: bool, max: bool) -> Self {
        Self::supported(ReasoningMode::Adaptive { xhigh, max })
    }

    #[must_use]
    pub fn unsupported() -> Self {
        Self::Unsupported
    }

    #[must_use]
    fn supported(mode: ReasoningMode) -> Self {
        Self::Supported {
            mode,
            can_disable: true,
        }
    }

    /// Mark reasoning as impossible to turn off.
    ///
    /// Such models reject an explicit "disable reasoning" request, so callers
    /// must omit the field instead of disabling it.
    #[must_use]
    pub fn always_on(self) -> Self {
        match self {
            Self::Supported { mode, .. } => Self::Supported {
                mode,
                can_disable: false,
            },
            Self::Unsupported => Self::Unsupported,
        }
    }

    /// Whether reasoning can be turned off entirely.
    ///
    /// Models that never reason report `true`: there is nothing to turn off, so
    /// an explicit disable is always safe.
    #[must_use]
    pub fn can_disable(&self) -> bool {
        match self {
            Self::Unsupported => true,
            Self::Supported { can_disable, .. } => *can_disable,
        }
    }

    /// The reasoning mode, or `None` when reasoning is unsupported.
    #[must_use]
    pub fn mode(&self) -> Option<ReasoningMode> {
        match self {
            Self::Unsupported => None,
            Self::Supported { mode, .. } => Some(*mode),
        }
    }

    #[must_use]
    pub fn min_tokens(&self) -> u32 {
        match self.mode() {
            Some(ReasoningMode::Budgetted { min_tokens, .. }) => min_tokens,
            _ => 0,
        }
    }

    #[must_use]
    pub fn max_tokens(&self) -> Option<u32> {
        match self.mode() {
            Some(ReasoningMode::Budgetted { max_tokens, .. }) => max_tokens,
            _ => None,
        }
    }

    /// Returns the lowest reasoning effort level supported by this model, if
    /// known.
    ///
    /// `Leveled` models return their lowest supported level.
    /// Other variants return `Option::None` — callers should decide how to
    /// handle "disable reasoning" for their provider (e.g. token budget 0,
    /// effort `minimal`, `thinking: disabled`, etc.).
    #[must_use]
    pub fn lowest_effort(&self) -> Option<ReasoningEffort> {
        match self {
            Self::Supported {
                can_disable,
                mode:
                    ReasoningMode::Leveled {
                        xlow,
                        low,
                        medium,
                        high,
                        xhigh,
                        max,
                    },
            } => {
                if *can_disable {
                    Some(ReasoningEffort::None)
                } else if *xlow {
                    Some(ReasoningEffort::Xlow)
                } else if *low {
                    Some(ReasoningEffort::Low)
                } else if *medium {
                    Some(ReasoningEffort::Medium)
                } else if *high {
                    Some(ReasoningEffort::High)
                } else if *xhigh {
                    Some(ReasoningEffort::XHigh)
                } else if *max {
                    Some(ReasoningEffort::Max)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Returns `true` if the model supports the `max` reasoning effort level.
    #[must_use]
    pub fn supports_max_effort(&self) -> bool {
        matches!(
            self.mode(),
            Some(
                ReasoningMode::Leveled { max: true, .. }
                    | ReasoningMode::Adaptive { max: true, .. }
            )
        )
    }

    #[must_use]
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported)
    }

    #[must_use]
    pub fn is_budgetted(&self) -> bool {
        matches!(self.mode(), Some(ReasoningMode::Budgetted { .. }))
    }

    #[must_use]
    pub fn is_leveled(&self) -> bool {
        matches!(self.mode(), Some(ReasoningMode::Leveled { .. }))
    }

    #[must_use]
    pub fn is_adaptive(&self) -> bool {
        matches!(self.mode(), Some(ReasoningMode::Adaptive { .. }))
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
