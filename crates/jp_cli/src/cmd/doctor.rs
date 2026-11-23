//! `jp doctor`: report what in a JP setup needs attention, and apply the fixes
//! the user names.
//!
//! [`Doctor::run`] is the entry point.
//! It runs none of the steps every other command runs first (the trash restore,
//! the startup scan and its cleanup), so the report describes the store as it
//! is, and nothing on disk changes unless `--fix` names a fix.
//!
//! The checks live with the data they cover; this module renders their findings
//! and applies the named fix through them.

use std::{borrow::Cow, collections::HashSet};

use clap::{Arg, ArgMatches, Command, builder::PossibleValuesParser};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_inquire::{InlineOption, InlineSelect};
use jp_printer::Printer;
use jp_storage::{
    backend::{Diagnosis, RecoveryBackend, Repair, RepairOutcome},
    validate::StorageLocation,
};
use serde::Serialize;
use tracing::warn;

use crate::{
    cli::CommandArgs,
    cmd::{self, Output},
    format::no_persist_note,
    load_failure::{self, LoadFailure},
    shared::confirm::{ConfirmFlag, decide},
};

/// `jp doctor`.
#[derive(Debug)]
pub(crate) struct Doctor {
    /// The fix `--fix` names, applied to every problem it covers.
    fix: Option<Repair>,

    confirm: ConfirmFlag,
}

impl CommandArgs for Doctor {
    fn augment(cmd: Command) -> Command {
        ConfirmFlag::augment(
            cmd.arg(
                Arg::new("fix")
                    .long("fix")
                    .value_name("NAME")
                    .value_parser(PossibleValuesParser::new(Repair::ALL.map(Repair::name)))
                    .help("Apply the named fix to every problem it covers"),
            ),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            fix: matches
                .get_one::<String>("fix")
                .and_then(|name| Repair::from_name(name)),
            confirm: ConfirmFlag::from_matches(matches)?,
        })
    }
}

impl Doctor {
    /// Report what `recovery` finds, or apply the fix `--fix` names.
    ///
    /// `interactive` says whether someone can answer the confirmation a fix
    /// asks for; `persists` is false under `--no-persist`, where `recovery`
    /// changes nothing.
    pub(crate) fn run(
        self,
        printer: &Printer,
        recovery: &dyn RecoveryBackend,
        interactive: bool,
        persists: bool,
    ) -> Output {
        let diagnosis = recovery.diagnose()?;
        let Some(fix) = self.fix else {
            print_report(printer, &Report::from(&diagnosis));
            return Ok(());
        };

        let result = apply(
            printer,
            recovery,
            &diagnosis,
            fix,
            self.confirm,
            interactive,
        );
        if !persists {
            printer.println("");
            printer.println(no_persist_note());
        }
        result
    }
}

/// What `jp doctor` reports.
pub(crate) struct Report<'a> {
    /// Conversations that do not load, one finding per stored copy.
    pub failures: Vec<Finding<'a>>,

    /// What the conversation store holds that is not a conversation.
    pub unrecognized: &'a [StorageLocation],

    /// Leftovers of interrupted writes, which the next command's startup scan
    /// removes.
    pub leftovers: &'a [StorageLocation],
}

/// A stored conversation that does not load, and the fix that makes it load.
pub(crate) struct Finding<'a> {
    pub failure: LoadFailure<'a>,
    pub fix: Option<Repair>,
}

impl<'a> From<&'a Diagnosis> for Report<'a> {
    fn from(diagnosis: &'a Diagnosis) -> Self {
        Self {
            failures: diagnosis
                .invalid
                .iter()
                .map(|diagnosed| Finding {
                    failure: LoadFailure::from(&diagnosed.conversation),
                    fix: diagnosed.repair,
                })
                .collect(),
            unrecognized: &diagnosis.unrecognized,
            leftovers: &diagnosis.leftovers,
        }
    }
}

/// Print the report on stdout: text, or one JSON record per problem.
fn print_report(printer: &Printer, report: &Report<'_>) {
    let format = printer.format();
    if format.is_json() {
        match render_json(report, format.is_json_pretty()) {
            Ok(records) => {
                for record in records {
                    printer.println_raw(record);
                }
                return;
            }
            Err(error) => warn!(%error, "Failed to serialize a report."),
        }
    }

    printer.println(render_text(report));
}

/// Render the text report, grouped by section.
pub(crate) fn render_text(report: &Report<'_>) -> String {
    let blocks: Vec<_> = [
        failures_block(&report.failures),
        unrecognized_block(report.unrecognized),
        leftovers_block(report.leftovers),
    ]
    .into_iter()
    .flatten()
    .collect();

    if blocks.is_empty() {
        return "No problems found.".to_owned();
    }

    format!("{}\n\n{}", "Conversations".bold(), blocks.join("\n\n"))
}

/// The conversations that do not load, listed once each by the first copy's
/// failure, then the fixes that make them load and the command that removes the
/// rest.
fn failures_block(findings: &[Finding<'_>]) -> Option<String> {
    let mut seen = HashSet::new();
    let findings: Vec<_> = findings
        .iter()
        .filter(|finding| seen.insert(finding.failure.id))
        .collect();
    if findings.is_empty() {
        return None;
    }

    let count = findings.len();
    let verb = if count == 1 { "fails" } else { "fail" };
    let entries = findings
        .iter()
        .map(|finding| {
            format!(
                "- {}: {}",
                finding.failure.id.to_string().bold().yellow(),
                finding.failure.error
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "{} {verb} to load:\n\n{entries}\n\n{}",
        counted(count, "conversation", "conversations")
            .bold()
            .yellow(),
        failure_hints(&findings).join("\n")
    ))
}

/// The commands that deal with `findings`: each fix that makes some of them
/// load, then the removal of the ones no fix covers.
fn failure_hints(findings: &[&Finding<'_>]) -> Vec<String> {
    let count = findings.len();
    let mut hints: Vec<_> = Repair::ALL
        .into_iter()
        .filter_map(|fix| {
            let covered = findings
                .iter()
                .filter(|finding| finding.fix == Some(fix))
                .count();
            let subject = match covered {
                0 => return None,
                _ if covered < count => format!("{covered} of them"),
                1 => "it".to_owned(),
                _ => "them".to_owned(),
            };
            Some(format!(
                "Run `jp doctor --fix {}` to make {subject} load.",
                fix.name()
            ))
        })
        .collect();

    let unfixable: Vec<_> = findings
        .iter()
        .filter(|finding| finding.fix.is_none())
        .map(|finding| finding.failure.id)
        .collect();
    if !unfixable.is_empty() {
        let subject = match unfixable.len() {
            _ if unfixable.len() < count => "the rest",
            1 => "it",
            _ => "them",
        };
        hints.push(format!(
            "Run `jp conversation rm {}` to remove {subject}.",
            joined_ids(&unfixable)
        ));
    }

    hints
}

/// What the store holds that is not a conversation, and the command that
/// removes it.
fn unrecognized_block(locations: &[StorageLocation]) -> Option<String> {
    if locations.is_empty() {
        return None;
    }

    let count = locations.len();
    let (verb, pronoun) = if count == 1 {
        ("is not a conversation", "it")
    } else {
        ("are not conversations", "them")
    };
    let paths: Vec<_> = locations
        .iter()
        .map(|location| quoted(location.as_str()))
        .collect();

    Some(format!(
        "{} in the conversation store {verb}:\n\n{}\n\nRun `rm -r {}` to remove {pronoun}.",
        counted(count, "directory", "directories").bold().yellow(),
        bullets(locations),
        paths.join(" ")
    ))
}

/// The leftovers of interrupted writes, which the next command removes.
fn leftovers_block(locations: &[StorageLocation]) -> Option<String> {
    if locations.is_empty() {
        return None;
    }

    let subject = if locations.len() == 1 {
        "1 leftover of an interrupted write".to_owned()
    } else {
        format!("{} leftovers of interrupted writes", locations.len())
    };

    Some(format!(
        "{}, which the next `jp` command removes:\n\n{}",
        subject.bold().yellow(),
        bullets(locations)
    ))
}

fn bullets(locations: &[StorageLocation]) -> String {
    locations
        .iter()
        .map(|location| format!("- {location}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `count` and the noun for it, e.g. `1 conversation` or `2 conversations`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn joined_ids(ids: &[ConversationId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` quoted for a POSIX shell when it needs quoting.
fn quoted(text: &str) -> String {
    shlex::try_quote(text).map_or_else(|_| text.to_owned(), Cow::into_owned)
}

/// Render one JSON record per problem.
///
/// A conversation that does not load gets the load-failure report's record,
/// with the fix that makes it load.
/// With `pretty`, each record is indented across several lines.
pub(crate) fn render_json(report: &Report<'_>, pretty: bool) -> serde_json::Result<Vec<String>> {
    let mut records = vec![];
    for finding in &report.failures {
        records.push(load_failure::render_record(
            &finding.failure,
            finding.fix.map(Repair::name),
            pretty,
        )?);
    }

    let located = [
        (report.unrecognized, UNRECOGNIZED),
        (report.leftovers, LEFTOVER),
    ];
    for (locations, kind) in located {
        for location in locations {
            let record = LocationRecord {
                level: kind.level,
                code: kind.code,
                message: kind.message,
                context: LocationContext { location },
            };
            records.push(if pretty {
                serde_json::to_string_pretty(&record)?
            } else {
                serde_json::to_string(&record)?
            });
        }
    }

    Ok(records)
}

/// The fixed fields of a problem found at a location.
#[derive(Clone, Copy)]
struct LocationKind {
    level: &'static str,
    code: &'static str,
    message: &'static str,
}

const UNRECOGNIZED: LocationKind = LocationKind {
    level: "warning",
    code: "not_a_conversation",
    message: "not a conversation",
};

const LEFTOVER: LocationKind = LocationKind {
    level: "info",
    code: "interrupted_write_leftover",
    message: "leftover of an interrupted write",
};

/// A problem found at a location, as a JSON record.
#[derive(Serialize)]
struct LocationRecord<'a> {
    level: &'static str,
    code: &'static str,
    message: &'static str,
    context: LocationContext<'a>,
}

#[derive(Serialize)]
struct LocationContext<'a> {
    location: &'a StorageLocation,
}

/// Apply `fix` to every conversation the diagnosis says it covers, after one
/// confirmation for all of them.
///
/// A conversation the fix cannot change is skipped, and the run ends with an
/// error naming each one.
fn apply(
    printer: &Printer,
    recovery: &dyn RecoveryBackend,
    diagnosis: &Diagnosis,
    fix: Repair,
    confirm: ConfirmFlag,
    interactive: bool,
) -> Output {
    let mut seen = HashSet::new();
    let ids: Vec<_> = diagnosis
        .invalid
        .iter()
        .filter(|diagnosed| diagnosed.repair == Some(fix))
        .map(|diagnosed| diagnosed.conversation.id)
        .filter(|id| seen.insert(*id))
        .collect();
    if ids.is_empty() {
        printer.println(format!("Nothing to fix with `{}`.", fix.name()));
        return Ok(());
    }

    let subject = counted(ids.len(), "conversation", "conversations");
    if confirm.preference() != Some(false) && !ask(printer, fix, &subject, interactive)? {
        printer.println("Nothing changed.");
        return Ok(());
    }

    let mut failed = vec![];
    for id in &ids {
        match recovery.repair(id, fix, &mut |_| true)? {
            RepairOutcome::Repaired => {}
            outcome => failed.push((*id, outcome)),
        }
    }

    let applied = ids.len() - failed.len();
    if applied > 0 {
        printer.println(format!(
            "Applied `{}` to {}.",
            fix.name(),
            counted(applied, "conversation", "conversations")
        ));
    }
    if failed.is_empty() {
        return Ok(());
    }

    let entries = failed
        .iter()
        .map(|(id, outcome)| format!("{id}:  {}", why_not(*outcome)))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Failed to apply `{}` to {} of {subject}:\n\n{entries}",
        fix.name(),
        failed.len()
    )
    .into())
}

/// Ask whether to apply `fix` to `subject`, the conversations it covers.
///
/// Fails when nobody can answer: writing to the user's conversations is never
/// assumed.
fn ask(
    printer: &Printer,
    fix: Repair,
    subject: &str,
    interactive: bool,
) -> Result<bool, cmd::Error> {
    if !interactive {
        return Err(format!(
            "applying `{}` to {subject} needs a confirmation and nobody is available to give one; \
             pass --no-confirm to apply it without asking",
            fix.name()
        )
        .into());
    }

    let options = vec![
        InlineOption::new('y', "yes, apply it"),
        InlineOption::new('n', "no, change nothing"),
    ];
    let select = InlineSelect::new(format!("Apply `{}` to {subject}?", fix.name()), options)
        .with_default('n');

    Ok(decide(select.prompt(&mut printer.prompt_writer()))?)
}

/// Why a repair left a conversation unchanged.
const fn why_not(outcome: RepairOutcome) -> &'static str {
    match outcome {
        RepairOutcome::Locked => "in use by another process",
        RepairOutcome::NotFound => "no longer stored",
        RepairOutcome::NotApplicable => "the fix no longer applies",
        RepairOutcome::Declined => "skipped",
        RepairOutcome::Repaired => "repaired",
    }
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
