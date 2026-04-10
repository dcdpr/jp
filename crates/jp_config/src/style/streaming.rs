//! Streaming response styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    fill::FillDefaults,
    partial::ToPartial,
    style::tool_call::{PartialProgressConfig, ProgressConfig},
};

/// Streaming response style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct StreamingConfig {
    /// Progress indicator configuration.
    ///
    /// Shows a waiting indicator while the LLM is processing the request.
    /// This covers the HTTP round-trip and time-to-first-token.
    #[setting(nested)]
    pub progress: ProgressConfig,
}

impl AssignKeyValue for PartialStreamingConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("progress") => self.progress.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialStreamingConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            progress: self.progress.delta(next.progress),
        }
    }
}

impl FillDefaults for PartialStreamingConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            progress: self.progress.fill_from(defaults.progress),
        }
    }
}

impl ToPartial for StreamingConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            progress: self.progress.to_partial(),
        }
    }
}
