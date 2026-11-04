//! Printing a verdict or a report.
//!
//! Every format carries rustqual's message for each finding, and the guidance
//! for its rule when there is any.

use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
};

use crate::{Finding, Report, Shared, Verdict};

/// How to print findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// Plain text, grouped by rule.
    #[default]
    Text,

    /// GitHub workflow commands, one annotation per finding.
    Github,

    /// A Markdown table, for a GitHub step summary.
    Markdown,

    /// Markdown grouped by rule, with the guidance after each group.
    Ai,

    /// The findings as JSON, without guidance.
    Json,
}

/// How to fix findings of each rule, the JP way.
///
/// Read from `.config/rustqual/guidance/`; see [`Guidance::file_name`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Guidance(BTreeMap<String, String>);

impl Guidance {
    /// The file the guidance for `rule` is read from: the rule ID with every
    /// `/` replaced by `_`, plus `.md`.
    #[must_use]
    pub fn file_name(rule: &str) -> String {
        format!("{}.md", rule.replace('/', "_"))
    }

    /// The guidance for `rule`, if there is any.
    #[must_use]
    pub fn get(&self, rule: &str) -> Option<&str> {
        self.0.get(rule).map(String::as_str)
    }
}

impl FromIterator<(String, String)> for Guidance {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(rule, text)| (rule, text.trim().to_owned()))
                .collect(),
        )
    }
}

/// Print what a comparison found.
#[must_use]
pub fn verdict(verdict: &Verdict, guidance: &Guidance, format: Format) -> String {
    if format == Format::Json {
        return json(verdict);
    }

    if verdict.passed() {
        return passed(verdict, format);
    }

    let new = verdict
        .new
        .iter()
        .map(|new| Item {
            finding: &new.finding,
            shared: new.shared,
        })
        .collect();
    let enforced = verdict
        .enforced
        .iter()
        .map(|finding| Item {
            finding,
            shared: None,
        })
        .collect();

    let sections = [
        Section {
            heading: "New rustqual findings",
            items: new,
        },
        Section {
            heading: "rustqual findings in enforced rules",
            items: enforced,
        },
    ];
    render(&sections, Level::Error, &verdict.titles, guidance, format)
}

/// Print every finding in a report.
#[must_use]
pub fn report(report: &Report, guidance: &Guidance, format: Format) -> String {
    if format == Format::Json {
        return json(&report.findings);
    }

    let items = report
        .findings
        .iter()
        .map(|finding| Item {
            finding,
            shared: None,
        })
        .collect();
    let section = Section {
        heading: "rustqual findings",
        items,
    };
    render(&[section], Level::Warning, &report.titles, guidance, format)
}

struct Section<'a> {
    heading: &'static str,
    items: Vec<Item<'a>>,
}

struct Item<'a> {
    finding: &'a Finding,
    shared: Option<Shared>,
}

#[derive(Clone, Copy)]
enum Level {
    Error,
    Warning,
}

impl Level {
    fn command(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

fn json(value: &impl serde::Serialize) -> String {
    // Serializing derived `Serialize` types into a string cannot fail.
    let mut out = serde_json::to_string_pretty(value).unwrap_or_default();
    out.push('\n');
    out
}

fn passed(verdict: &Verdict, format: Format) -> String {
    let counts = verdict
        .head
        .iter()
        .map(|(pass, head)| {
            let base = verdict.base.get(pass).copied().unwrap_or(0);
            format!("{pass} {base} -> {head}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let line = format!("No new rustqual findings. Findings per pass, base -> head: {counts}.");

    match format {
        Format::Github => format!("::notice::{}\n", escape_data(&line)),
        _ => format!("{line}\n"),
    }
}

/// `level` is the severity GitHub annotations carry; other formats have none.
fn render(
    sections: &[Section<'_>],
    level: Level,
    titles: &BTreeMap<String, String>,
    guidance: &Guidance,
    format: Format,
) -> String {
    let sections = sections.iter().filter(|section| !section.items.is_empty());
    let mut out = String::new();
    let mut empty = true;
    for section in sections {
        empty = false;
        // Writing to a `String` cannot fail.
        let _ = match format {
            Format::Text => text(&mut out, section, titles, guidance),
            Format::Github => github(&mut out, section, level, titles, guidance),
            Format::Markdown => markdown(&mut out, section, titles, guidance),
            Format::Ai => ai(&mut out, section, titles, guidance),
            Format::Json => Ok(()),
        };
    }

    if empty {
        out.push_str("No rustqual findings.\n");
    }
    out
}

fn text(
    out: &mut String,
    section: &Section<'_>,
    titles: &BTreeMap<String, String>,
    guidance: &Guidance,
) -> fmt::Result {
    if !out.is_empty() {
        out.push('\n');
    }
    writeln!(out, "{}: {}", section.heading, section.items.len())?;

    for (rule, items) in by_rule(&section.items) {
        writeln!(out, "\n{}", rule_heading(rule, titles))?;
        for item in items {
            let finding = item.finding;
            match finding.symbol.as_str() {
                "" => writeln!(out, "  {}: {}", location(finding), finding.message)?,
                symbol => writeln!(
                    out,
                    "  {} ({symbol}): {}",
                    location(finding),
                    finding.message
                )?,
            }
            if let Some(shared) = item.shared {
                writeln!(out, "    {}", shared_note(shared))?;
            }
        }
        if let Some(text) = guidance.get(rule) {
            writeln!(out)?;
            for line in text.lines() {
                writeln!(out, "  {line}")?;
            }
        }
    }
    Ok(())
}

fn ai(
    out: &mut String,
    section: &Section<'_>,
    titles: &BTreeMap<String, String>,
    guidance: &Guidance,
) -> fmt::Result {
    if !out.is_empty() {
        out.push('\n');
    }
    writeln!(out, "# {} ({})", section.heading, section.items.len())?;

    for (rule, items) in by_rule(&section.items) {
        writeln!(out, "\n## {}\n", rule_heading(rule, titles))?;
        for item in items {
            let finding = item.finding;
            match finding.symbol.as_str() {
                "" => writeln!(out, "- `{}`: {}", location(finding), finding.message)?,
                symbol => writeln!(
                    out,
                    "- `{}` in `{symbol}`: {}",
                    location(finding),
                    finding.message
                )?,
            }
            if let Some(shared) = item.shared {
                writeln!(out, "  {}", shared_note(shared))?;
            }
        }
        if let Some(text) = guidance.get(rule) {
            writeln!(out, "\n{text}")?;
        }
    }
    Ok(())
}

fn markdown(
    out: &mut String,
    section: &Section<'_>,
    titles: &BTreeMap<String, String>,
    guidance: &Guidance,
) -> fmt::Result {
    if !out.is_empty() {
        out.push('\n');
    }
    writeln!(out, "### {} ({})\n", section.heading, section.items.len())?;
    writeln!(out, "| Location | Rule | Symbol | Message |")?;
    writeln!(out, "| --- | --- | --- | --- |")?;

    for item in &section.items {
        let finding = item.finding;
        let symbol = match finding.symbol.as_str() {
            "" => String::new(),
            symbol => format!("`{}`", cell(symbol)),
        };
        let mut message = cell(&finding.message);
        if let Some(shared) = item.shared {
            write!(message, " ({})", shared_note(shared))?;
        }
        writeln!(
            out,
            "| `{}` | {} | {symbol} | {message} |",
            cell(&location(finding)),
            cell(&finding.rule)
        )?;
    }

    for rule in by_rule(&section.items).keys() {
        let Some(text) = guidance.get(rule) else {
            continue;
        };
        writeln!(
            out,
            "\n<details>\n<summary>{}</summary>\n\n{text}\n\n</details>",
            rule_heading(rule, titles)
        )?;
    }
    Ok(())
}

fn github(
    out: &mut String,
    section: &Section<'_>,
    level: Level,
    titles: &BTreeMap<String, String>,
    guidance: &Guidance,
) -> fmt::Result {
    for item in &section.items {
        let finding = item.finding;

        let mut properties = Vec::new();
        if !finding.file.is_empty() {
            properties.push(format!("file={}", escape_property(&finding.file)));
        }
        if finding.line > 0 {
            properties.push(format!("line={}", finding.line));
        }
        properties.push(format!(
            "title={}",
            escape_property(&rule_heading(&finding.rule, titles))
        ));

        let mut message = finding.message.clone();
        if let Some(shared) = item.shared {
            write!(message, "\n\n{}", shared_note(shared))?;
        }
        if let Some(text) = guidance.get(&finding.rule) {
            write!(message, "\n\n{text}")?;
        }

        writeln!(
            out,
            "::{} {}::{}",
            level.command(),
            properties.join(","),
            escape_data(&message)
        )?;
    }
    Ok(())
}

/// The items of a section grouped by rule, each group in finding order.
fn by_rule<'s, 'a>(items: &'s [Item<'a>]) -> BTreeMap<&'a str, Vec<&'s Item<'a>>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for item in items {
        groups
            .entry(item.finding.rule.as_str())
            .or_default()
            .push(item);
    }
    groups
}

fn rule_heading(rule: &str, titles: &BTreeMap<String, String>) -> String {
    match titles.get(rule) {
        Some(title) => format!("{rule}: {title}"),
        None => rule.to_owned(),
    }
}

fn location(finding: &Finding) -> String {
    match (finding.file.as_str(), finding.line) {
        ("", _) => "<workspace>".to_owned(),
        (file, 0) => file.to_owned(),
        (file, line) => format!("{file}:{line}"),
    }
}

fn shared_note(shared: Shared) -> String {
    format!(
        "{} findings share this rule and symbol, {} more than at the base.",
        shared.of, shared.new
    )
}

/// Make `text` fit in one Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// Escape the message of a GitHub workflow command.
fn escape_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escape a property value of a GitHub workflow command.
fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
