use indoc::formatdoc;

use super::auth;
use crate::{
    Context, Result,
    github::{ORG, REPO, handle_404},
    util::{
        ToolResult,
        diff::{colored_diff, text_diff, unified_diff},
        error, preview,
    },
};

pub(crate) async fn github_pr_edit(
    ctx: Context,
    pull_number: u64,
    title: Option<String>,
    description: Option<String>,
) -> ToolResult {
    let edit = match Edit::new(title, description) {
        Ok(edit) => edit,
        Err(msg) => return error(msg),
    };

    if ctx.action.is_format_arguments() {
        return Ok(format_for_approval(pull_number, &edit).await.into());
    }

    auth().await?;

    let mut request = jp_github::instance().pulls(ORG, REPO).update(pull_number);
    if let Some(title) = &edit.title {
        request = request.title(title);
    }
    if let Some(description) = &edit.description {
        request = request.body(description);
    }

    let pull = request
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    let url = pull
        .html_url
        .map_or_else(String::new, |url| format!(" ({url})"));

    Ok(format!("Updated the {} of PR #{pull_number}{url}.", edit.fields()).into())
}

/// A requested change to a pull request's title and description.
///
/// At least one field is set, and a set title is a single non-empty line with
/// surrounding whitespace trimmed.
#[derive(Debug, PartialEq)]
struct Edit {
    title: Option<String>,
    description: Option<String>,
}

impl Edit {
    fn new(
        title: Option<String>,
        description: Option<String>,
    ) -> std::result::Result<Self, &'static str> {
        if title.is_none() && description.is_none() {
            return Err("Provide `title`, `description`, or both.");
        }

        let title = match title {
            None => None,
            Some(title) => {
                let title = title.trim();
                if title.is_empty() {
                    return Err("`title` must not be empty.");
                }
                if title.contains('\n') {
                    return Err("`title` must be a single line.");
                }
                Some(title.to_owned())
            }
        };

        Ok(Self { title, description })
    }

    fn fields(&self) -> &'static str {
        match (&self.title, &self.description) {
            (Some(_), Some(_)) => "title and description",
            (Some(_), None) => "title",
            (None, _) => "description",
        }
    }
}

/// A pull request's title and description as GitHub currently stores them.
struct PullText {
    title: String,
    description: String,
}

async fn format_for_approval(pull_number: u64, edit: &Edit) -> String {
    // Fetched at approval time rather than trusting what the assistant read
    // earlier, so the diff shows what the edit overwrites right now, including
    // any change someone made on GitHub in the meantime.
    match fetch_pull_text(pull_number).await {
        Ok(current) => render_edit_preview(pull_number, &current, edit),
        Err(e) => render_unverified_preview(pull_number, edit, &e.to_string()),
    }
}

async fn fetch_pull_text(pull_number: u64) -> Result<PullText> {
    auth().await?;

    let pull = jp_github::instance()
        .pulls(ORG, REPO)
        .get(pull_number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    Ok(PullText {
        title: pull.title.unwrap_or_default(),
        description: pull.body.unwrap_or_default(),
    })
}

/// Lay out the approval preview as one diff per edited field, against the pull
/// request's current text.
fn render_edit_preview(pull_number: u64, current: &PullText, edit: &Edit) -> String {
    let mut sections = vec![format!("PR #{pull_number} \u{2014} edit {}", edit.fields())];

    if let Some(title) = &edit.title {
        sections.push(render_field_diff("title", &current.title, title));
    }
    if let Some(description) = &edit.description {
        sections.push(render_field_diff(
            "description",
            &current.description,
            description,
        ));
    }

    sections.join("\n\n")
}

fn render_field_diff(label: &str, old: &str, new: &str) -> String {
    let old = normalize_lines(old);
    let new = normalize_lines(new);
    if old == new {
        return format!("{label}: unchanged");
    }

    let diff = text_diff(&old, &new);
    let unified = unified_diff(&diff, label);
    colored_diff(&diff, &unified, label)
}

/// Convert CRLF line endings to LF and end the text with a newline.
///
/// Descriptions edited in GitHub's web UI are stored with CRLF line endings.
/// Without this, every line of such a description would show as changed.
fn normalize_lines(text: &str) -> String {
    let mut text = text.replace("\r\n", "\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Lay out the approval preview when the current text could not be fetched: the
/// new title and description in a blockquote rail, with the fetch error in the
/// header.
fn render_unverified_preview(pull_number: u64, edit: &Edit, reason: &str) -> String {
    let text = [edit.title.as_deref(), edit.description.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    let quoted = preview(&text);

    formatdoc!(
        "
        PR #{pull_number} \u{2014} edit {fields} (current text unavailable: {reason})

        {quoted}",
        fields = edit.fields(),
    )
}

#[cfg(test)]
#[path = "pr_edit_tests.rs"]
mod tests;
