use jp_config::PartialAppConfig;
use serde::{Deserialize, Serialize};

/// A configuration delta event - represents a change in conversation configuration.
///
/// This is a delta event, meaning it is merged on top of all previous `ConfigDelta`
/// events in the stream. Any non-config events before the first `ConfigDelta` are
/// considered to have the default configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDelta(pub Box<PartialAppConfig>);

impl ConfigDelta {
    #[must_use]
    pub fn new(config: PartialAppConfig) -> Self {
        Self(Box::new(config))
    }

    #[must_use]
    pub fn into_inner(self) -> PartialAppConfig {
        *self.0
    }
}

impl From<PartialAppConfig> for ConfigDelta {
    fn from(config: PartialAppConfig) -> Self {
        Self::new(config)
    }
}

impl From<ConfigDelta> for PartialAppConfig {
    fn from(delta: ConfigDelta) -> Self {
        delta.into_inner()
    }
}

impl AsRef<PartialAppConfig> for ConfigDelta {
    fn as_ref(&self) -> &PartialAppConfig {
        &self.0
    }
}

impl std::ops::Deref for ConfigDelta {
    type Target = PartialAppConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ConfigDelta {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
