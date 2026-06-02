//! Render a parsed agentic-shepherd issue into markdown.
//!
//! The whole module is pure: it takes a deserialized [`IssueDetail`] and
//! produces a markdown string with no I/O, which keeps it testable without the
//! `agentic-shepherd` binary.

use crate::model::{
    ChecklistItem, CodeBlock, Commit, Continuation, IssueDetail, IssueResolution, Item,
    PrefixedItem, ProgressChecklist, Section,
};

pub(crate) fn render(detail: &IssueDetail) -> String {
    let issue = &detail.issue;

    let mut out = String::new();
    out.push_str(&format!(
        "# Issue {}: {}\n\n",
        detail.issue_id,
        issue.title.trim()
    ));
    out.push_str(&format!("Source: ag://issues/{}\n", detail.issue_id));
    out.push_str(&format!("File:   {}\n", detail.file));
    out.push_str(&format!(
        "Opened: {}, {}\n",
        issue.timestamp.user.name, issue.timestamp.datetime
    ));

    if !issue.description.is_empty() {
        out.push_str("\n## Description\n\n");
        render_outline(&issue.description, 0, &mut out);
    }

    // Reading order: what it is, how we'll do it, what happened, how it ended.
    // Resolution (catharsis) renders last.
    render_section(&mut out, "Analysis", issue.analysis_section.as_ref());
    render_section(
        &mut out,
        "Implementation Plan",
        issue.implementation_plan_section.as_ref(),
    );
    render_section(
        &mut out,
        "Progress Notes",
        issue.progress_notes_section.as_ref(),
    );
    render_section(
        &mut out,
        "Implementation Details",
        issue.implementation_details_section.as_ref(),
    );
    render_section(
        &mut out,
        "Testing Results",
        issue.testing_results_section.as_ref(),
    );
    render_section(&mut out, "Debugging", issue.debugging_section.as_ref());

    if let Some(catharsis) = &issue.catharsis_section
        && !catharsis.commits.is_empty()
    {
        out.push_str("\n## Resolution\n\n");
        render_resolution(&catharsis.commits, &mut out);
    }

    out
}

fn render_section(out: &mut String, heading: &str, section: Option<&Section>) {
    let Some(section) = section else { return };
    if section.is_empty() {
        return;
    }

    out.push_str(&format!("\n## {heading}\n\n"));
    if let Some(ts) = &section.timestamp {
        out.push_str(&format!("_{}, {}_\n\n", ts.user.name, ts.datetime));
    }
    render_outline(&section.content, 0, out);

    render_extra(out, "Root Causes", section.root_causes.as_deref());
    render_extra(out, "Steps", section.steps.as_deref());
    render_extra(out, "Symptoms", section.symptoms.as_deref());
    render_extra(out, "Debugging Plan", section.debugging_plan.as_deref());

    if let Some(checklist) = &section.checklist {
        render_checklist(out, checklist);
    }
    if let Some(results) = &section.test_results {
        render_test_results(out, results);
    }
}

fn render_extra(out: &mut String, heading: &str, items: Option<&[Item]>) {
    let Some(items) = items.filter(|items| !items.is_empty()) else {
        return;
    };
    out.push_str(&format!("\n### {heading}\n\n"));
    render_outline(items, 0, out);
}

/// Render a list of outline items as nested markdown bullets.
///
/// Nesting comes from the `continuations` tree, not from the indentation
/// metadata in the source: a `Sublist` recurses one level deeper, a
/// `WrappedParagraph` folds back into the parent bullet, and a `CodeBlock`
/// renders as a fenced block under the bullet.
fn render_outline(items: &[Item], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for item in items {
        let prefix = item
            .timestamp
            .as_ref()
            .map(|t| format!("({}, {}) ", t.user.name, t.datetime))
            .unwrap_or_default();
        out.push_str(&format!("{indent}- {prefix}{}\n", item_line(item)));
        render_block_continuations(&item.continuations, depth + 1, out);
    }
}

/// Build a bullet's text, folding any `WrappedParagraph` continuations back
/// into the line they soft-wrapped from.
fn item_line(item: &Item) -> String {
    let mut line = item.content.trim().to_owned();
    for cont in &item.continuations {
        if let Continuation::WrappedParagraph(p) = cont {
            let text = p.content.trim();
            if !text.is_empty() {
                line.push(' ');
                line.push_str(text);
            }
        }
    }
    line
}

fn render_block_continuations(continuations: &[Continuation], depth: usize, out: &mut String) {
    for cont in continuations {
        match cont {
            Continuation::Sublist(sublist) => render_outline(&sublist.items, depth, out),
            Continuation::CodeBlock(code) => render_code_block(code, depth, out),
            Continuation::WrappedParagraph(_) => {}
        }
    }
}

fn render_code_block(code: &CodeBlock, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let language = code.language.as_deref().unwrap_or_default();
    out.push_str(&format!("{indent}```{language}\n"));
    for line in code.content.lines() {
        out.push_str(&format!("{indent}{line}\n"));
    }
    out.push_str(&format!("{indent}```\n"));
}

fn render_checklist(out: &mut String, checklist: &ProgressChecklist) {
    let named = [
        &checklist.diagnostic_logging_added,
        &checklist.root_cause_identified,
        &checklist.fix_implemented,
        &checklist.tests_updated,
        &checklist.edge_cases_handled,
        &checklist.code_formatted,
        &checklist.clippy_warnings_fixed,
        &checklist.tests_passing,
        &checklist.implementation_complete,
        &checklist.documentation_updated,
        &checklist.review_requested,
        &checklist.commit_prepared,
    ];

    let rendered: Vec<&ChecklistItem> = named
        .into_iter()
        .chain(&checklist.custom_items)
        .filter(|item| !item.description.trim().is_empty())
        .collect();

    if rendered.is_empty() {
        return;
    }

    out.push_str("\n### Checklist\n\n");
    for item in rendered {
        let mark = if item.completed { "x" } else { " " };
        out.push_str(&format!("- [{mark}] {}\n", item.description.trim()));
    }
}

fn render_test_results(out: &mut String, results: &[(String, bool)]) {
    if results.is_empty() {
        return;
    }
    out.push_str("\n### Test Results\n\n");
    for (name, passed) in results {
        let mark = if *passed { "pass" } else { "FAIL" };
        out.push_str(&format!("- [{mark}] {name}\n"));
    }
}

fn render_resolution(commits: &[IssueResolution], out: &mut String) {
    for resolution in commits {
        match resolution {
            IssueResolution::ClosedWithCommit(commit) => render_commit(out, "Closed in", commit),
            IssueResolution::Obsoleted(commit) => render_commit(out, "Obsoleted in", commit),
            IssueResolution::InProgress(ts) => {
                out.push_str(&format!(
                    "- In progress ({}, {})\n",
                    ts.user.name, ts.datetime
                ));
            }
            IssueResolution::Tabled(ts) => {
                out.push_str(&format!("- Tabled ({}, {})\n", ts.user.name, ts.datetime));
            }
            IssueResolution::ClosedByFiat(item) => render_prefixed(out, "Closed", item),
            IssueResolution::Reopened(item) => render_prefixed(out, "Reopened", item),
        }
    }
}

fn render_commit(out: &mut String, verb: &str, commit: &Commit) {
    out.push_str(&format!(
        "- {verb} {} ({}, {})\n",
        commit.hash, commit.timestamp.user.name, commit.timestamp.datetime
    ));
    if let Some(comments) = &commit.comments {
        render_outline(comments, 1, out);
    }
}

fn render_prefixed(out: &mut String, verb: &str, prefixed: &PrefixedItem) {
    let when = prefixed
        .item
        .timestamp
        .as_ref()
        .map(|t| format!(" ({}, {})", t.user.name, t.datetime))
        .unwrap_or_default();
    out.push_str(&format!(
        "- {verb}: {} {}{when}\n",
        prefixed.prefix.trim(),
        item_line(&prefixed.item)
    ));
    render_block_continuations(&prefixed.item.continuations, 1, out);
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
