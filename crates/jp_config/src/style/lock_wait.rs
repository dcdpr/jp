//! Lock-wait progress indicator configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Progress indicator shown while waiting for a conversation lock held by
/// another session.
///
/// When a conversation is locked by another process, the CLI polls for the
/// lock to be released. This configuration controls the timer indicator
/// displayed during that wait.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct LockWaitConfig {
    /// Whether to show the waiting indicator.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing the waiting indicator.
    ///
    /// During this initial period, the CLI polls silently.
    /// Set to 0 to show the indicator immediately.
    #[setting(default = 1)]
    pub delay_secs: u32,

    /// Interval in milliseconds between timer updates.
    #[setting(default = 100)]
    pub interval_ms: u32,
}

impl AssignKeyValue for PartialLockWaitConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "show" => self.show = kv.try_some_bool()?,
            "delay_secs" => self.delay_secs = kv.try_some_u32()?,
            "interval_ms" => self.interval_ms = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLockWaitConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
        }
    }
}

impl ToPartial for LockWaitConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
            delay_secs: partial_opt(&self.delay_secs, defaults.delay_secs),
            interval_ms: partial_opt(&self.interval_ms, defaults.interval_ms),
        }
    }
}
