use serde::Serialize;

/// Per-table schema as emitted at the top of each `<node>` in a
/// `trace-query-result`.
#[derive(Debug, Clone, Serialize)]
pub struct Schema {
    pub name: String,
    pub documentation: Option<String>,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    /// Short identifier (e.g. `time`, `core-index`).
    pub mnemonic: String,
    /// Human-readable label (e.g. `Timestamp`, `Core Index`).
    pub name: String,
    /// xctrace's internal type tag (e.g. `sample-time`, `kperf-bt`).
    pub engineering_type: EngineeringType,
}

/// xctrace's internal type tag (e.g. `sample-time`, `kperf-bt`).
/// Open-ended because Apple adds new tags between Xcode releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EngineeringType(String);

impl EngineeringType {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl From<String> for EngineeringType {
    fn from(s: String) -> Self {
        Self(s)
    }
}
