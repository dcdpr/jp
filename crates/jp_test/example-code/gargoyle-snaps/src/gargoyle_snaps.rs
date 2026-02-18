//! Always-on snapshot testing utilities with path normalization filters.
//!
//! This crate provides filters for normalizing paths and timestamps in
//! insta snapshots, ensuring reproducible tests across different environments.
//!
//! # The Independent Verification Principle
//!
//! **Accepting a snapshot must be an independent verification.**
//!
//! The `expected_output` parameter must be a human-written statement of what
//! correctness looks like, based on your understanding of the requirements—NOT
//! derived from the code being tested.
//!
//! Why this matters: If `expected_output` is generated from the code (e.g., by
//! echoing computed results), and the code has a bug, then the guidance will
//! also be wrong. The reviewer has no independent reference point.
//!
//! When a reviewer accepts a snapshot, they're checking: "Does the actual
//! output match what we independently know should be correct?" This only
//! works if `expected_output` comes from human understanding of requirements,
//! not from the code under test.

use insta::Settings;
use insta::_macro_support::{assert_snapshot as insta_assert_snapshot, SnapshotValue};
use onlyerror::Error;
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

/// Identifies a test by its fn name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestName(pub &'static str);

/// First 7 of a Git commit hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHashShort(pub &'static str);

/// A single file conflict within a fixture.
#[derive(Debug, Clone)]
pub struct FileConflict {
    pub fixture: Fixture,
    pub file: PathBuf,
    pub readers: Vec<TestName>,
    pub writers: Vec<TestName>,
}

/// All conflicts found during verification.
#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("File access conflicts: {0:?}")]
    Conflicts(Vec<FileConflict>),
}

/// File access mode for concurrency checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Test only reads this file.
    Read,

    /// Test reads and writes this file.
    ReadWrite,
}

/// Identifies a test fixture (shared test environment).
#[derive(Debug, Clone)]
pub struct Fixture {
    pub name: &'static str,
    pub commit: CommitHashShort,
    pub path: PathBuf,
}

/// Declares which files a test accesses within a fixture.
#[derive(Debug, Clone)]
pub struct ConcurrencyInfo {
    pub test_name: TestName,
    pub fixture: Fixture,
    pub files: Vec<(PathBuf, AccessMode)>,
}

inventory::collect!(ConcurrencyInfo);

/// Checks for conflicts between tests accessing the same fixture files.
/// Returns Ok(()) if no conflicts, or Err with all conflicts found.
pub fn verify_no_conflicts() -> Result<(), ConflictError> {
    let mut by_fixture: HashMap<&str, Vec<&ConcurrencyInfo>> = HashMap::new();

    for info in inventory::iter::<ConcurrencyInfo> {
        by_fixture.entry(info.fixture.name).or_default().push(info);
    }

    let mut conflicts = Vec::new();

    for (_fixture_name, tests) in by_fixture {
        let mut file_accesses: HashMap<PathBuf, Vec<(&ConcurrencyInfo, AccessMode)>> =
            HashMap::new();

        for test in &tests {
            for (file, access) in &test.files {
                file_accesses
                    .entry(file.clone())
                    .or_default()
                    .push((test, *access));
            }
        }

        for (file, accesses) in file_accesses {
            if accesses.len() > 1 {
                let has_write = accesses.iter().any(|(_, a)| *a == AccessMode::ReadWrite);
                if has_write {
                    let readers: Vec<TestName> = accesses
                        .iter()
                        .filter(|(_, a)| *a == AccessMode::Read)
                        .map(|(info, _)| info.test_name.clone())
                        .collect();
                    let writers: Vec<TestName> = accesses
                        .iter()
                        .filter(|(_, a)| *a == AccessMode::ReadWrite)
                        .map(|(info, _)| info.test_name.clone())
                        .collect();
                    let fixture = accesses[0].0.fixture.clone();
                    conflicts.push(FileConflict {
                        fixture,
                        file,
                        readers,
                        writers,
                    });
                }
            }
        }
    }

    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(ConflictError::Conflicts(conflicts))
    }
}

/// Returns the snapshots directory for the calling crate.
#[macro_export]
macro_rules! snapshots_dir {
    () => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/snapshots")
    };
}

/// JSON snapshot with filters, using caller's snapshots directory.
///
/// Wraps insta's snapshot with path/timestamp normalization filters.
/// Snapshots are stored in `$CARGO_MANIFEST_DIR/snapshots/`.
///
/// # Arguments
///
/// * `name` - Unique identifier for this snapshot within the test file.
///   Combined with the test file stem to form the full snapshot filename.
///   Example: `"parsed_issue"` in `issue_test.rs` → `issue__parsed_issue.snap`
///
/// The purposes of `desc`, `test_purpose`, and `expected_output` do not overlap.
/// To use any of them correctly, you must understand all three and ensure your
/// content belongs in exactly one.
///
/// * `desc` - Brief sentence answering "WHAT is this test about?"
///   Name the functionality being tested, not the data format.
///   Keep it laconic - under 10 words. Don't say "Test" - they know it's a test.
///   Example: `"Prioritize-issue moves #3 to top."` NOT `"prioritize-issue JSON response"`
///
/// * `test_purpose` - Complete sentence explaining WHY this test exists.
///   State the invariant or requirement being verified.
///   Be concise - no "This test verifies that" boilerplate.
///   Example: `"Nested continuations must survive round-trip parsing."`
///
/// * `expected_output` - Concrete field values that indicate correctness.
///   **Must be human-written, independent of the code under test.**
///   Tells the reviewer exactly what to check in the snapshot data.
///   Be specific: name fields, expected values, structural properties.
///   NEVER generate this dynamically from test inputs or code—if the code
///   is wrong, dynamic guidance will be wrong too.
///   Example: `"issue_id=42, title contains 'test', description has 2 items,
///             first continuation has 3 subitems"`
///
/// * `result` - The value to snapshot (must be serializable)
///
/// # Reviewer Flow
///
/// When someone reviews a snapshot, they read in order:
/// 1. `desc` → "What am I looking at?"
/// 2. `test_purpose` → "Why does this matter? Where should I look?"
/// 3. `expected_output` → "What specific values should I verify?"
/// 4. Compare against actual snapshot content
///
/// # Example
///
/// ```ignore
/// assert_snapshot_json!(
///     "parsed_issue",
///     "Parse issue #42 from markdown.",
///     "Section boundaries must be respected. \
///      Check Analysis section is separate from description.",
///     "issue_id=42, description has 1 item, analysis_section is Some",
///     &parsed_issue,
/// );
/// ```
#[macro_export]
macro_rules! assert_snapshot_json {
    ($name:expr, $desc:expr, $test_purpose:expr, $expected_output:expr, $result:expr $(,)?) => {
        $crate::assert_snapshot_impl_json(
            env!("CARGO_MANIFEST_DIR"),
            file!(),
            module_path!(),
            line!(),
            $name,
            $desc,
            $test_purpose,
            $expected_output,
            stringify!($result),
            $result,
        )
    };
}

/// Text snapshot with filters, using caller's snapshots directory.
///
/// Wraps insta's snapshot with path/timestamp normalization filters.
/// Snapshots are stored in `$CARGO_MANIFEST_DIR/snapshots/`.
///
/// # Arguments
///
/// * `name` - Unique identifier for this snapshot within the test file.
///   Combined with the test file stem to form the full snapshot filename.
///   Example: `"formatted_output"` in `format_test.rs` → `format__formatted_output.snap`
///
/// The purposes of `desc`, `test_purpose`, and `expected_output` do not overlap.
/// To use any of them correctly, you must understand all three and ensure your
/// content belongs in exactly one.
///
/// * `desc` - Brief sentence answering "WHAT is this test about?"
///   Name the functionality being tested, not the data format.
///   Keep it laconic - under 10 words. Don't say "Test" - they know it's a test.
///   Example: `"Prettyprint issue with catharsis section."`
///
/// * `test_purpose` - Complete sentence explaining WHY this test exists.
///   State the invariant or requirement being verified.
///   Be concise - no "This test verifies that" boilerplate.
///   Example: `"Lines must wrap at 99 chars with proper continuation indentation."`
///
/// * `expected_output` - Concrete field values that indicate correctness.
///   **Must be human-written, independent of the code under test.**
///   Tells the reviewer exactly what to check in the snapshot data.
///   Be specific: name fields, expected values, structural properties.
///   NEVER generate this dynamically from test inputs or code—if the code
///   is wrong, dynamic guidance will be wrong too.
///   Example: `"No line exceeds 99 chars, continuation lines start with 4 spaces,
///             timestamp appears on first line"`
///
/// * `result` - The string value to snapshot
///
/// # Reviewer Flow
///
/// When someone reviews a snapshot, they read in order:
/// 1. `desc` → "What am I looking at?"
/// 2. `test_purpose` → "Why does this matter? Where should I look?"
/// 3. `expected_output` → "What specific values should I verify?"
/// 4. Compare against actual snapshot content
///
/// # Example
///
/// ```ignore
/// assert_snapshot_text!(
///     "wrapped_output",
///     "Line wrapping for long description item.",
///     "Autowrap must preserve content while respecting line limits. \
///      Check continuation line indentation.",
///     "Lines under 99 chars, continuations indented 4 spaces",
///     &formatted_text,
/// );
/// ```
#[macro_export]
macro_rules! assert_snapshot_text {
    ($name:expr, $desc:expr, $test_purpose:expr, $expected_output:expr, $result:expr $(,)?) => {
        $crate::assert_snapshot_impl_text(
            env!("CARGO_MANIFEST_DIR"),
            file!(),
            module_path!(),
            line!(),
            $name,
            $desc,
            $test_purpose,
            $expected_output,
            stringify!($result),
            $result,
        )
    };
}

/// Debug snapshot with filters, using caller's snapshots directory.
///
/// Wraps insta's debug snapshot with path/timestamp normalization filters.
/// Snapshots are stored in `$CARGO_MANIFEST_DIR/snapshots/`.
///
/// Use this for types that implement `Debug` but not `Serialize`.
///
/// # Arguments
///
/// * `name` - Unique identifier for this snapshot within the test file.
///   Combined with the test file stem to form the full snapshot filename.
///   Example: `"parsed_struct"` in `parser_test.rs` → `parser__parsed_struct.snap`
///
/// The purposes of `desc`, `test_purpose`, and `expected_output` do not overlap.
/// To use any of them correctly, you must understand all three and ensure your
/// content belongs in exactly one.
///
/// * `desc` - Brief sentence answering "WHAT is this test about?"
///   Name the functionality being tested, not the data format.
///   Keep it laconic - under 10 words. Don't say "Test" - they know it's a test.
///   Example: `"Parse nested bullets into Issue struct."`
///
/// * `test_purpose` - Complete sentence explaining WHY this test exists.
///   State the invariant or requirement being verified.
///   Be concise - no "This test verifies that" boilerplate.
///   Example: `"Continuations must parse into nested structure."`
///
/// * `expected_output` - Concrete field values that indicate correctness.
///   **Must be human-written, independent of the code under test.**
///   Tells the reviewer exactly what to check in the snapshot data.
///   Be specific: name fields, expected values, structural properties.
///   NEVER generate this dynamically from test inputs or code—if the code
///   is wrong, dynamic guidance will be wrong too.
///   Example: `"description.len()=2, first item has 1 continuation,
///             continuation contains 3 subitems"`
///
/// * `result` - The value to snapshot (must implement Debug)
///
/// # Reviewer Flow
///
/// When someone reviews a snapshot, they read in order:
/// 1. `desc` → "What am I looking at?"
/// 2. `test_purpose` → "Why does this matter? Where should I look?"
/// 3. `expected_output` → "What specific values should I verify?"
/// 4. Compare against actual snapshot content
///
/// # Example
///
/// ```ignore
/// assert_snapshot_debug!(
///     "nested_issue",
///     "Deeply nested continuation structure.",
///     "Three levels of nesting must be preserved. \
///      Check continuations at each level.",
///     "description[0].continuations.len()=1, sublist has 2 items, \
///      second subitem has its own continuations",
///     &parsed_issue,
/// );
/// ```
#[macro_export]
macro_rules! assert_snapshot_debug {
    ($name:expr, $desc:expr, $test_purpose:expr, $expected_output:expr, $result:expr $(,)?) => {
        $crate::assert_snapshot_impl_debug(
            env!("CARGO_MANIFEST_DIR"),
            file!(),
            module_path!(),
            line!(),
            $name,
            $desc,
            $test_purpose,
            $expected_output,
            stringify!($result),
            $result,
        )
    };
}

/// Standard timestamp filters for snapshot tests.
///
/// This function provides regex filters to replace timestamps with a placeholder
/// so snapshots remain stable across test runs.
pub fn timestamp_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // ISO 8601 with Z timezone
        (
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z",
            "[timestamp]",
        ),
        // ISO 8601 with timezone offset
        (
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?[+-]\d{2}:\d{2}",
            "[timestamp]",
        ),
        // ISO 8601 without timezone (local time, no Z or offset)
        (
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?",
            "[timestamp]",
        ),
        // SQL datetime format (common in SQLite)
        (
            r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?",
            "[timestamp]",
        ),
        // Full timestamped prefix with username (e.g., [claude 20251108 14:30 UTC])
        // NOTE: Must come before the plain timestamp pattern to match correctly
        (
            r"\[[\w-]+ \d{8} \d{2}:\d{2} UTC\]",
            "[USERNAME YYYYMMDD HH:MM UTC]",
        ),
        // Issue timestamp format (YYYYMMDD HH:MM UTC)
        (r"\d{8} \d{2}:\d{2} UTC", "YYYYMMDD HH:MM UTC"),
    ]
}

/// Filters for normalizing temporary directory paths in snapshots.
pub fn temp_path_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // macOS temp directories (with optional /private prefix)
        (r"/private/var/folders/[^/]+/[^/]+/[^/]+/[^/]+/", "[tmp]/"),
        (r"/var/folders/[^/]+/[^/]+/[^/]+/[^/]+/", "[tmp]/"),
        // Claude Code sandbox temp directories (with optional /private prefix)
        (r"/private/tmp/claude/[^/]+/", "[tmp]/"),
        (r"/tmp/claude/[^/]+/", "[tmp]/"),
        // Linux/generic temp directories
        (r"/tmp\.[^/]+/", "[tmp]/"),
        // Generic .tmp* patterns
        (r"\.tmp[A-Za-z0-9]+/", "[tmp]/"),
    ]
}

/// Filters for normalizing Debug struct datetime fields.
///
/// These handle the `datetime: "..."` pattern that appears in Debug output
/// of structs containing DateTime fields.
pub fn debug_datetime_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // Debug struct datetime format (datetime: "2025-01-01T...")
        (r#"datetime: "[^"]+""#, r#"datetime: "[DATETIME]""#),
    ]
}

/// Filters for normalizing home directory paths in snapshots.
///
/// Replaces `/Users/username/` or `/home/username/` with `[HOME]/`
/// so snapshots are portable across machines.
pub fn home_path_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // macOS home directories (/Users/username/)
        (r"/Users/[^/]+/", "[HOME]/"),
        // Linux home directories (/home/username/)
        (r"/home/[^/]+/", "[HOME]/"),
    ]
}

/// Combined filters for typical snapshot testing needs.
///
/// Includes timestamp, temporary path, debug datetime, and home path normalization.
pub fn usual_filters() -> Vec<(&'static str, &'static str)> {
    let mut filters = timestamp_filters();
    filters.extend(temp_path_filters());
    filters.extend(debug_datetime_filters());
    filters.extend(home_path_filters());
    filters
}

/// Recursively sorts all object keys in a JSON value alphabetically.
///
/// This ensures deterministic JSON output regardless of the order in which
/// keys were inserted into serde_json::Value objects.
fn sort_json_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, sort_json_keys(v)))
                .collect();
            Value::Object(sorted.into_iter().collect::<Map<String, Value>>())
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_json_keys).collect()),
        other => other,
    }
}

/// Serializes a value to pretty-printed JSON with sorted keys.
fn serialize_json_sorted<T: serde::Serialize>(value: &T) -> String {
    let json_value: Value =
        serde_json::to_value(value).expect("Failed to convert to serde_json::Value");
    let sorted = sort_json_keys(json_value);
    serde_json::to_string_pretty(&sorted).expect("Failed to serialize sorted JSON")
}

/// Runs snapshot assertion with standardized insta settings and correct source attribution.
///
/// Builds snapshot name from source_file (e.g., "tests/integration/json_execution_test.rs"
/// becomes "json_execution") and combines with snapshot_name.
///
/// Calls insta's internal `assert_snapshot` directly to pass the correct source_file,
/// ensuring snapshot metadata shows the actual test file, not this library.
#[allow(clippy::too_many_arguments, clippy::absolute_paths)]
fn run_snapshot_assertion(
    manifest_dir: &'static str,
    source_file: &'static str,
    module_path: &'static str,
    line: u32,
    snapshot_name: &str,
    desc: &str,
    test_purpose: &str,
    expected_output: &str,
    expression: &str,
    content: &str,
) {
    assert!(
        test_purpose.len() > 4,
        "test_purpose must be more than 4 characters"
    );
    assert!(
        expected_output.len() > 4,
        "expected_output must be more than 4 characters"
    );

    let stem = Path::new(source_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("source_file should have a valid file stem");
    let prefix = stem.strip_suffix("_test").unwrap_or(stem);
    let full_name = format!("{prefix}__{snapshot_name}");

    let snapshot_path = format!("{manifest_dir}/snapshots");

    let info_value = json!({
        "test_purpose": test_purpose,
        "expected_output": expected_output
    });

    let mut settings = Settings::clone_current();
    settings.set_description(desc);
    settings.set_info(&info_value);
    settings.set_snapshot_path(&snapshot_path);
    settings.set_prepend_module_to_snapshot(false);
    for (pattern, replacement) in usual_filters() {
        settings.add_filter(pattern, replacement);
    }

    settings.bind(|| {
        let snapshot_value = SnapshotValue::FileText {
            name: Some(Cow::Borrowed(&full_name)),
            content,
        };

        // Get workspace root (same logic as insta's _get_workspace_root! macro)
        let workspace = insta::_macro_support::get_cargo_workspace(
            insta::_macro_support::Workspace::DetectWithCargo(manifest_dir),
        );

        insta_assert_snapshot(
            snapshot_value,
            workspace.as_path(),
            module_path, // function_name - insta uses module_path for this
            module_path,
            source_file, // This is the key fix - pass actual test file
            line,
            expression,
        )
        .unwrap();
    });
}

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn assert_snapshot_impl_json<T: serde::Serialize>(
    manifest_dir: &'static str,
    source_file: &'static str,
    module_path: &'static str,
    line: u32,
    snapshot_name: &str,
    desc: &str,
    test_purpose: &str,
    expected_output: &str,
    expression: &str,
    result: T,
) {
    let content = serialize_json_sorted(&result);
    run_snapshot_assertion(
        manifest_dir,
        source_file,
        module_path,
        line,
        snapshot_name,
        desc,
        test_purpose,
        expected_output,
        expression,
        &content,
    );
}

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn assert_snapshot_impl_text(
    manifest_dir: &'static str,
    source_file: &'static str,
    module_path: &'static str,
    line: u32,
    snapshot_name: &str,
    desc: &str,
    test_purpose: &str,
    expected_output: &str,
    expression: &str,
    result: impl AsRef<str>,
) {
    run_snapshot_assertion(
        manifest_dir,
        source_file,
        module_path,
        line,
        snapshot_name,
        desc,
        test_purpose,
        expected_output,
        expression,
        result.as_ref(),
    );
}

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn assert_snapshot_impl_debug<T: Debug>(
    manifest_dir: &'static str,
    source_file: &'static str,
    module_path: &'static str,
    line: u32,
    snapshot_name: &str,
    desc: &str,
    test_purpose: &str,
    expected_output: &str,
    expression: &str,
    result: T,
) {
    let content = format!("{:#?}", result);
    run_snapshot_assertion(
        manifest_dir,
        source_file,
        module_path,
        line,
        snapshot_name,
        desc,
        test_purpose,
        expected_output,
        expression,
        &content,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filters_not_empty() {
        assert!(!timestamp_filters().is_empty());
        assert!(!temp_path_filters().is_empty());
        assert!(!usual_filters().is_empty());
    }

    #[test]
    fn test_usual_filters_combines_all() {
        let usual = usual_filters();
        let timestamps = timestamp_filters();
        let temps = temp_path_filters();
        let debug_dt = debug_datetime_filters();
        let home = home_path_filters();
        assert_eq!(
            usual.len(),
            timestamps.len() + temps.len() + debug_dt.len() + home.len()
        );
    }
}
