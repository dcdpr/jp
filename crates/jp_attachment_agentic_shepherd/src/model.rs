//! Deserialization types for `agentic-shepherd`'s `IssueDetail` JSON output.
//!
//! These mirror the subset of the tracker's schema that the handler renders.
//! Fields the renderer doesn't use (such as `indentation`) are simply not
//! declared and are ignored during deserialization.
//! Optional and collection fields default, so absent or `null` values never
//! fail the parse.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDetail {
    pub(crate) issue: Issue,
    pub(crate) file: String,
    pub(crate) issue_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Issue {
    pub(crate) timestamp: Timestamp,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: Vec<Item>,
    #[serde(default)]
    pub(crate) catharsis_section: Option<CatharsisSection>,
    #[serde(default)]
    pub(crate) analysis_section: Option<Section>,
    #[serde(default)]
    pub(crate) implementation_plan_section: Option<Section>,
    #[serde(default)]
    pub(crate) progress_notes_section: Option<Section>,
    #[serde(default)]
    pub(crate) implementation_details_section: Option<Section>,
    #[serde(default)]
    pub(crate) testing_results_section: Option<Section>,
    #[serde(default)]
    pub(crate) debugging_section: Option<Section>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Timestamp {
    pub(crate) user: User,
    pub(crate) datetime: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct User {
    pub(crate) name: String,
}

/// One outline item.
///
/// Covers both the tracker's `DescriptionItem` (no timestamp) and
/// `TimestampedDescriptionItem` (timestamp present): the optional `timestamp`
/// absorbs the difference.
#[derive(Debug, Deserialize)]
pub(crate) struct Item {
    #[serde(default)]
    pub(crate) timestamp: Option<Timestamp>,
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) continuations: Vec<Continuation>,
}

#[derive(Debug, Deserialize)]
pub(crate) enum Continuation {
    CodeBlock(CodeBlock),
    Sublist(Sublist),
    WrappedParagraph(WrappedParagraph),
}

#[derive(Debug, Deserialize)]
pub(crate) struct CodeBlock {
    #[serde(default)]
    pub(crate) language: Option<String>,
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Sublist {
    #[serde(default)]
    pub(crate) items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WrappedParagraph {
    pub(crate) content: String,
}

/// A content section (analysis, implementation plan, progress notes, ...).
///
/// The tracker models these as distinct types that share a `content` list and
/// differ only in their section-specific extras.
/// A single struct with every extra optional deserializes all of them; absent
/// extras default to `None`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Section {
    #[serde(default)]
    pub(crate) timestamp: Option<Timestamp>,
    #[serde(default)]
    pub(crate) content: Vec<Item>,
    #[serde(default)]
    pub(crate) root_causes: Option<Vec<Item>>,
    #[serde(default)]
    pub(crate) steps: Option<Vec<Item>>,
    #[serde(default)]
    pub(crate) symptoms: Option<Vec<Item>>,
    #[serde(default)]
    pub(crate) debugging_plan: Option<Vec<Item>>,
    #[serde(default)]
    pub(crate) checklist: Option<ProgressChecklist>,
    #[serde(default)]
    pub(crate) test_results: Option<Vec<(String, bool)>>,
}

impl Section {
    pub(crate) fn is_empty(&self) -> bool {
        self.content.is_empty()
            && self.root_causes.is_none()
            && self.steps.is_none()
            && self.symptoms.is_none()
            && self.debugging_plan.is_none()
            && self.checklist.is_none()
            && self.test_results.is_none()
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatharsisSection {
    #[serde(default)]
    pub(crate) commits: Vec<IssueResolution>,
}

#[derive(Debug, Deserialize)]
pub(crate) enum IssueResolution {
    InProgress(Timestamp),
    Tabled(Timestamp),
    ClosedWithCommit(Commit),
    Obsoleted(Commit),
    ClosedByFiat(PrefixedItem),
    Reopened(PrefixedItem),
}

#[derive(Debug, Deserialize)]
pub(crate) struct Commit {
    pub(crate) timestamp: Timestamp,
    pub(crate) hash: String,
    #[serde(default)]
    pub(crate) comments: Option<Vec<Item>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PrefixedItem {
    pub(crate) prefix: String,
    pub(crate) item: Item,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProgressChecklist {
    #[serde(default)]
    pub(crate) diagnostic_logging_added: ChecklistItem,
    #[serde(default)]
    pub(crate) root_cause_identified: ChecklistItem,
    #[serde(default)]
    pub(crate) fix_implemented: ChecklistItem,
    #[serde(default)]
    pub(crate) tests_updated: ChecklistItem,
    #[serde(default)]
    pub(crate) edge_cases_handled: ChecklistItem,
    #[serde(default)]
    pub(crate) code_formatted: ChecklistItem,
    #[serde(default)]
    pub(crate) clippy_warnings_fixed: ChecklistItem,
    #[serde(default)]
    pub(crate) tests_passing: ChecklistItem,
    #[serde(default)]
    pub(crate) implementation_complete: ChecklistItem,
    #[serde(default)]
    pub(crate) documentation_updated: ChecklistItem,
    #[serde(default)]
    pub(crate) review_requested: ChecklistItem,
    #[serde(default)]
    pub(crate) commit_prepared: ChecklistItem,
    #[serde(default)]
    pub(crate) custom_items: Vec<ChecklistItem>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChecklistItem {
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) completed: bool,
}
