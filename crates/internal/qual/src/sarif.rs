//! Parsing `rustqual --format sarif` output.
//!
//! SARIF is the only `rustqual` output format that includes the stable rule ID
//! (`CX-001`, `DRY-002`, `BP-009`) on every finding.
//! Because SARIF results do not include the enclosing Rust symbol,
//! [`crate::symbol`] resolves symbols from the source files.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A SARIF result before symbol resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub rule: String,

    /// Path relative to the analyzed directory, or empty for workspace-wide
    /// findings (such as dependency cycles).
    pub file: String,

    /// 1-based line number, or `0` when the finding has no line location.
    pub line: usize,

    pub message: String,
}

/// Parsed findings and rule titles from a single SARIF report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Sarif {
    pub findings: Vec<RawFinding>,

    /// Map from rule ID to its `shortDescription` text.
    pub titles: BTreeMap<String, String>,
}

/// Error parsing or validating a `rustqual` SARIF report.
#[derive(Debug, thiserror::Error)]
pub enum SarifError {
    #[error("not a SARIF report: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SARIF version {0:?} is not supported; expected 2.1.0")]
    Version(String),

    #[error("expected one run in the SARIF report, found {0}")]
    Runs(usize),

    #[error("the SARIF report was written by {0:?}, not rustqual")]
    Tool(String),
}

/// Parse a rustqual SARIF report.
///
/// # Errors
///
/// Returns an error when `input` is not SARIF 2.1.0 with exactly one run
/// written by rustqual, or when any result lacks a rule ID, a message, or its
/// list of locations.
pub fn parse(input: &str) -> Result<Sarif, SarifError> {
    let log: Log = serde_json::from_str(input)?;
    if log.version != "2.1.0" {
        return Err(SarifError::Version(log.version));
    }

    let [run] = <[Run; 1]>::try_from(log.runs).map_err(|runs| SarifError::Runs(runs.len()))?;
    if run.tool.driver.name != "rustqual" {
        return Err(SarifError::Tool(run.tool.driver.name));
    }

    let titles = run
        .tool
        .driver
        .rules
        .into_iter()
        .filter_map(|rule| Some((rule.id, rule.short_description?.text)))
        .collect();

    let findings = run
        .results
        .into_iter()
        .map(|result| {
            let location = result.locations.into_iter().next();
            let (file, line) = location.map_or((String::new(), 0), |location| {
                let physical = location.physical_location;
                let line = physical.region.map_or(0, |region| region.start_line);
                (physical.artifact_location.uri, line)
            });

            RawFinding {
                rule: result.rule_id,
                file,
                line,
                message: result.message.text,
            }
        })
        .collect();

    Ok(Sarif { findings, titles })
}

#[derive(Deserialize)]
struct Log {
    version: String,
    runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    tool: Tool,
    results: Vec<ResultRow>,
}

#[derive(Deserialize)]
struct Tool {
    driver: Driver,
}

#[derive(Deserialize)]
struct Driver {
    name: String,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    id: String,
    short_description: Option<Text>,
}

#[derive(Deserialize)]
struct Text {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResultRow {
    rule_id: String,
    message: Text,
    locations: Vec<Location>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: PhysicalLocation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: ArtifactLocation,
    region: Option<Region>,
}

#[derive(Deserialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: usize,
}

#[cfg(test)]
#[path = "sarif_tests.rs"]
mod tests;
