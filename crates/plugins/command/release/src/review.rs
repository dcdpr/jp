//! `jp release review`: the assistant's opinion of a release's notes and
//! upgrade guide.
//!
//! What people wrote is reviewed: each fragment's entry and upgrade section,
//! each highlight, and a release's `overview.md`.
//! What rendering adds around them, the credits, the headings, and the summary
//! that stands in for a missing overview, is not.
//!
//! [`query`] asks the host for the review as a structured `query`, in a
//! conversation that expires, and [`findings`] reads what came back.
//! Each [`Finding`] names the file to fix; nothing is edited.

use changelog::directory::Contents;
use jp_plugin::message::QueryRequest;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// How long the conversation a review runs in is kept.
const LIFETIME: &str = "5m";

/// What a review runs under: a conversation kept out of the workspace tree, no
/// reasoning, and no tool calls, which a turn run for a plugin would make
/// without asking.
pub(crate) const CFG: [&str; 3] = [
    "conversation.start_local=true",
    "assistant.model.parameters.reasoning=off",
    "assistant.tool_choice=none",
];

/// The problems a review reports, by the id a [`Finding`] names them with.
const KINDS: [&str; 4] = [
    "commit_message",
    "no_user_impact",
    "no_upgrade_steps",
    "typo",
];

/// What the model is asked, ahead of the release.
pub(crate) const PROMPT: &str = concat!(
    "Review the release notes and the upgrade guide below. They are written for the people ",
    "who use the software, not for the people who work on its code.\n",
    "\n",
    "Report these problems, and nothing else:\n",
    "\n",
    "- `commit_message`: an entry that reads like a commit message, describing the change to ",
    "the code instead of what users notice.\n",
    "- `no_user_impact`: an entry that doesn't say what changed for users.\n",
    "- `no_upgrade_steps`: an upgrade section that doesn't say what users have to do.\n",
    "- `typo`: a typo, in any part.\n",
    "\n",
    "The release is given part by part, each naming its `file`. An `<entry>` is a fragment's ",
    "line in the notes, listed under its `type`, and its `<upgrade>` is its section of the ",
    "upgrade guide. A `<highlight>` is a section of the notes about one feature, headed by its ",
    "title. The `<overview>` opens the notes.\n",
    "\n",
    "For each problem, give the `file` of the part it is in, quote the words it is in exactly, ",
    "and say in one or two sentences how to fix it. Return no findings when there are none.",
);

/// What a review reads: a release, or the fragments no release lists yet, with
/// each file's path shown from the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subject {
    /// The release's version; `None` for the fragments no release lists yet.
    pub version: Option<Version>,

    /// The release's `overview.md`, when it has one: the file's path, and what
    /// it holds.
    pub overview: Option<(String, String)>,

    /// The fragments and highlights, each `path` shown from the workspace root.
    pub contents: Contents,
}

impl Subject {
    /// Whether there is nothing to review.
    pub(crate) fn is_empty(&self) -> bool {
        self.overview.is_none()
            && self.contents.fragments.is_empty()
            && self.contents.highlights.is_empty()
    }

    /// What the review is of, as a sentence names it.
    pub(crate) fn name(&self) -> String {
        match &self.version {
            Some(version) => format!("the notes and upgrade guide of {version}"),
            None => "the unreleased fragments".to_owned(),
        }
    }
}

/// A problem the review found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    /// The file the problem is in, shown from the workspace root.
    pub file: String,

    /// Which problem it is: one of [`KINDS`], or whatever the model wrote
    /// instead, since not every provider holds a response to the schema's
    /// `enum`.
    pub kind: String,

    /// The words the problem is in.
    pub quote: String,

    /// How to fix it.
    pub comment: String,
}

/// The query that asks for a review of `subject`.
///
/// The release goes in the request rather than as an attachment, so the review
/// reads the files as they are on disk now, without an attachment handler.
pub(crate) fn query(subject: &Subject) -> QueryRequest {
    QueryRequest {
        id: Some("review".to_owned()),
        conversation: String::new(),
        content: format!("{PROMPT}\n\n{}", release(subject)),
        new: true,
        title: Some(match &subject.version {
            Some(version) => format!("Review of release {version}"),
            None => "Review of the unreleased changes".to_owned(),
        }),
        cfg: CFG.map(ToOwned::to_owned).to_vec(),
        schema: Some(schema()),
        expires_in: Some(LIFETIME.to_owned()),
    }
}

/// `subject` as the model reads it: each part in a tag that names its file.
fn release(subject: &Subject) -> String {
    let version = subject
        .version
        .as_ref()
        .map_or_else(|| "unreleased".to_owned(), ToString::to_string);
    let mut text = format!("<release version=\"{version}\">\n");

    if let Some((file, overview)) = &subject.overview {
        text.push_str(&format!(
            "<overview file=\"{file}\">\n{}\n</overview>\n",
            overview.trim_end()
        ));
    }

    for highlight in &subject.contents.highlights {
        text.push_str(&format!(
            "<highlight file=\"{}\">\n{}\n</highlight>\n",
            highlight.path,
            section(&highlight.file.title, &highlight.file.body)
        ));
    }

    for fragment in &subject.contents.fragments {
        let entry = &fragment.file;
        text.push_str(&format!(
            "<entry file=\"{}\" type=\"{}\">\n{}\n",
            fragment.path,
            entry.type_,
            entry.entry.trim_end()
        ));
        if let Some(upgrade) = &entry.upgrade {
            text.push_str(&format!(
                "<upgrade>\n{}\n</upgrade>\n",
                section(&upgrade.title, &upgrade.body)
            ));
        }
        text.push_str("</entry>\n");
    }

    text.push_str("</release>\n");
    text
}

/// A heading and the Markdown below it, the way the notes show a section.
fn section(title: &str, body: &str) -> String {
    let body = body.trim_end();
    if body.is_empty() {
        format!("## {title}")
    } else {
        format!("## {title}\n\n{body}")
    }
}

/// The shape the review comes back in.
fn schema() -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "description": "One item per problem; empty when there are none",
                "items": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "The `file` of the part the problem is in, as the release names it"
                        },
                        "kind": {
                            "type": "string",
                            "enum": KINDS,
                            "description": "Which problem it is"
                        },
                        "quote": {
                            "type": "string",
                            "description": "The words the problem is in, quoted exactly"
                        },
                        "comment": {
                            "type": "string",
                            "description": "How to fix it, in one or two sentences"
                        }
                    },
                    "required": ["file", "kind", "quote", "comment"]
                }
            }
        },
        "required": ["findings"]
    });

    schema.as_object().cloned().unwrap_or_default()
}

/// The findings in the structured response a review finished with.
///
/// The error says why the response holds none.
pub(crate) fn findings(data: Option<Value>) -> Result<Vec<Finding>, String> {
    #[derive(Deserialize)]
    struct Response {
        findings: Vec<Finding>,
    }

    let data = data.ok_or("the response carried no structured data")?;

    serde_json::from_value::<Response>(data)
        .map(|response| response.findings)
        .map_err(|error| format!("the response didn't match the schema: {error}"))
}

/// The findings as `review` prints them: by file, in the order the files first
/// appear, each with the words it is about and how to fix them.
///
/// Says so when there are none, naming what was reviewed as `subject`.
pub(crate) fn report(findings: &[Finding], subject: &str) -> String {
    if findings.is_empty() {
        return format!("The assistant found nothing to change in {subject}.\n");
    }

    let mut files: Vec<(&str, Vec<&Finding>)> = vec![];
    for finding in findings {
        match files
            .iter_mut()
            .find(|(file, _)| *file == finding.file.as_str())
        {
            Some((_, found)) => found.push(finding),
            None => files.push((finding.file.as_str(), vec![finding])),
        }
    }

    let sections: Vec<String> = files
        .iter()
        .map(|(file, found)| {
            let mut section = format!("{file}\n");
            for finding in found {
                section.push_str(&format!(
                    "- {}: \"{}\"\n  {}\n",
                    label(&finding.kind),
                    one_line(&finding.quote),
                    one_line(&finding.comment)
                ));
            }
            section
        })
        .collect();

    sections.join("\n")
}

/// The findings as `review` prints them for `jp --format json`: `{"findings":
/// [...]}`, each finding's keys in the order [`Finding`] declares them.
pub(crate) fn report_json(findings: &[Finding]) -> String {
    #[derive(Serialize)]
    struct Report<'a> {
        findings: &'a [Finding],
    }

    serde_json::to_string(&Report { findings }).expect("findings serialize to JSON")
}

/// How the report names a finding's kind.
fn label(kind: &str) -> &str {
    match kind {
        "commit_message" => "Reads like a commit message",
        "no_user_impact" => "Doesn't say what changed for users",
        "no_upgrade_steps" => "Doesn't say what to do",
        "typo" => "Typo",
        other => other,
    }
}

/// `text` on one line, so a finding stays one item of the report.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
