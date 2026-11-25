//! `jp doctor`: diagnose problems in the conversation store and apply named
//! fixes.
//!
//! [`Doctor::run`] is the entry point.
//! Unlike regular commands, `jp doctor` skips automatic trash restoration and
//! startup cleanup so it can report the store's exact on-disk state without
//! modifying anything unless `--fix` is passed.

use std::{borrow::Cow, collections::HashSet, str::FromStr as _};

use clap::{Arg, ArgAction, ArgMatches, Command, builder::PossibleValuesParser};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::{
    backend::{
        Diagnosis, RecoveryBackend, Repair, RepairOutcome, StoredConversation, StrayEntry,
        StrayKind,
    },
    validate::StorageLocation,
};
use jp_term::width::display_width;
use serde::Serialize;
use tracing::warn;

use crate::{
    cli::CommandArgs,
    cmd::{self, Output},
    format::no_persist_note,
    load_failure::{self, LoadFailure},
    shared::confirm::{self, Answer, Answers, ConfirmFlag, Prompt, heading, stored_details},
};

/// Exit status returned when `jp doctor` finds one or more problems.
///
/// Distinct from general runtime errors (`1`) and CLI usage errors (`2`).
const PROBLEMS_FOUND_EXIT_STATUS: u8 = 3;

#[derive(Debug)]
pub(crate) struct Doctor {
    fix: Option<Repair>,

    /// Target conversation IDs from `--id` (or empty to target all applicable
    /// conversations).
    ids: Vec<ConversationId>,

    confirm: ConfirmFlag,
}

impl CommandArgs for Doctor {
    fn augment(cmd: Command) -> Command {
        ConfirmFlag::augment(
            cmd.after_long_help("Exits with status 3 when the report lists a problem.")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .value_name("NAME")
                        .value_parser(PossibleValuesParser::new(Repair::ALL.map(Repair::name)))
                        .help("Apply the named fix to every problem it covers"),
                )
                .arg(
                    Arg::new("id")
                        .long("id")
                        .value_name("ID")
                        .action(ArgAction::Append)
                        .requires("fix")
                        .value_parser(|id: &str| {
                            ConversationId::from_str(id).map_err(|error| error.to_string())
                        })
                        .help("Apply the fix only to this conversation; repeat for more"),
                ),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            fix: matches
                .get_one::<String>("fix")
                .and_then(|name| Repair::from_name(name)),
            ids: matches
                .get_many::<ConversationId>("id")
                .into_iter()
                .flatten()
                .copied()
                .collect(),
            confirm: ConfirmFlag::from_matches(matches)?,
        })
    }
}

impl Doctor {
    /// Diagnose the store via `recovery`, or apply the repair selected by
    /// `--fix`.
    ///
    /// Exits with [`PROBLEMS_FOUND_EXIT_STATUS`] when reporting and at least
    /// one problem is found.
    pub(crate) fn run(
        self,
        printer: &Printer,
        recovery: &dyn RecoveryBackend,
        interactive: bool,
        persists: bool,
    ) -> Output {
        let Some(repair) = self.fix else {
            return self.run_with(printer, recovery, None, persists);
        };

        let prompt = fix_prompt(repair);
        let pretty = printer.pretty_printing_enabled();
        let mut ask = |stored: &StoredConversation, more: bool| -> Result<Answer, cmd::Error> {
            let details =
                stored_details(stored, pretty).with_heading(heading("Fixing", stored.id, pretty));
            Ok(confirm::ask(printer, &prompt, &details.to_string(), more)?)
        };

        let ask = interactive.then_some(&mut ask as &mut Ask<'_>);
        self.run_with(printer, recovery, ask, persists)
    }

    /// Implementation of [`Self::run`] with an injectable confirmation
    /// callback.
    fn run_with(
        self,
        printer: &Printer,
        recovery: &dyn RecoveryBackend,
        ask: Option<&mut Ask<'_>>,
        persists: bool,
    ) -> Output {
        let diagnosis = recovery.diagnose()?;
        let Some(fix) = self.fix else {
            let report = Report::from(&diagnosis);
            print_report(printer, &report);
            if report.is_empty() {
                return Ok(());
            }
            return Err(cmd::Error::from(PROBLEMS_FOUND_EXIT_STATUS).expected());
        };

        let fix = Fix {
            repair: fix,
            ids: self.ids,
            confirm: self.confirm,
        };
        let result = fix.apply(printer, recovery, &diagnosis, ask);
        if !persists {
            printer.println("");
            printer.println(no_persist_note());
        }
        result
    }
}

/// Findings collected for a `jp doctor` report.
pub(crate) struct Report<'a> {
    /// Conversations that failed validation, one entry per invalid stored copy.
    pub failures: Vec<Finding<'a>>,

    /// Non-conversation entries and leftover temporary files in the store.
    pub strays: &'a [StrayEntry],
}

/// An invalid conversation paired with its applicable repair, if any.
pub(crate) struct Finding<'a> {
    pub failure: LoadFailure<'a>,
    pub fix: Option<Repair>,

    /// `true` if ID-based commands resolve to another copy of the conversation,
    /// so this copy must be removed by path.
    pub shadowed: bool,
}

impl Report<'_> {
    fn is_empty(&self) -> bool {
        self.failures.is_empty() && self.strays.is_empty()
    }
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
                    shadowed: diagnosed.shadowed,
                })
                .collect(),
            strays: &diagnosis.strays,
        }
    }
}

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

/// Severity level of a diagnostic row, used to color its target column.
#[derive(Clone, Copy)]
enum Severity {
    /// A conversation is missing data or cannot be read.
    Error,

    /// An unrecognized directory or leftover temporary file in the store.
    Warning,
}

/// Orange in the 256-color ANSI palette.
const WARNING_COLOR: Color = Color::AnsiValue(208);

/// A single row in the text report table.
struct Row {
    severity: Severity,
    target: String,
    problem: String,
    fix: String,
}

/// Render the text report, grouping findings by section and appending bulk
/// `--fix` hints when a repair applies to multiple conversations.
pub(crate) fn render_text(report: &Report<'_>) -> String {
    let live: Vec<_> = conversation_rows(&report.failures, false)
        .chain(report.strays.iter().map(stray_row))
        .collect();
    let archived: Vec<_> = conversation_rows(&report.failures, true).collect();

    let sections: Vec<_> = [
        ("Conversations", live),
        ("Archived conversations", archived),
    ]
    .into_iter()
    .filter(|(_, rows)| !rows.is_empty())
    .map(|(title, rows)| format!("## {title}\n{}", table(&rows)))
    .collect();
    if sections.is_empty() {
        return "No problems found.".to_owned();
    }

    let mut text = format!("# Report\n{}", sections.join("\n"));
    let hints = fix_all_hints(&report.failures);
    if !hints.is_empty() {
        text.push_str("\n\n");
        text.push_str(&hints.join("\n"));
    }
    text
}

/// Return a hint line for each repair that applies to more than one
/// conversation.
fn fix_all_hints(findings: &[Finding<'_>]) -> Vec<String> {
    let mut seen = HashSet::new();
    let fixes: Vec<_> = findings
        .iter()
        .filter(|finding| seen.insert(finding.failure.id))
        .filter_map(|finding| finding.fix)
        .collect();

    Repair::ALL
        .into_iter()
        .filter_map(|repair| {
            let count = fixes.iter().filter(|fix| **fix == repair).count();
            (count > 1).then(|| {
                format!(
                    "Run `jp doctor --fix {}` to fix all {count} conversations it covers.",
                    repair.name()
                )
            })
        })
        .collect()
}

fn table(rows: &[Row]) -> String {
    let target_width = rows.iter().map(|row| display_width(&row.target)).max();
    let problem_width = rows.iter().map(|row| display_width(&row.problem)).max();
    let pad = |text: &str, width: Option<usize>| {
        " ".repeat(width.unwrap_or_default() - display_width(text))
    };

    rows.iter()
        .map(|row| {
            let target = match row.severity {
                Severity::Error => row.target.as_str().bold().red(),
                Severity::Warning => row.target.as_str().bold().with(WARNING_COLOR),
            };
            format!(
                "- {target}{}  {}{}    {}",
                pad(&row.target, target_width),
                row.problem,
                pad(&row.problem, problem_width),
                row.fix
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build one table row per unique conversation ID in the given partition
/// (`archived`), using the first failing copy's error.
///
/// Each shadowed copy gets its own row with an `rm -r <path>` command, because
/// an ID-based command would act on the live copy.
fn conversation_rows<'a>(
    findings: &'a [Finding<'_>],
    archived: bool,
) -> impl Iterator<Item = Row> + 'a {
    let mut seen = HashSet::new();
    findings
        .iter()
        .filter(move |finding| finding.failure.archived == archived)
        .filter(move |finding| finding.shadowed || seen.insert(finding.failure.id))
        .map(|finding| {
            let id = finding.failure.id;
            let fix = match finding.fix {
                _ if finding.shadowed => {
                    format!("rm -r {}", quoted(finding.failure.location.as_str()))
                }
                Some(repair) => format!("jp doctor --id={id} --fix {}", repair.name()),
                None => format!("jp c rm {id}"),
            };

            Row {
                severity: Severity::Error,
                target: id.to_string(),
                problem: finding.failure.error.to_string(),
                fix,
            }
        })
}

fn stray_row(stray: &StrayEntry) -> Row {
    Row {
        severity: Severity::Warning,
        target: stray.name.clone(),
        problem: stray_kind(stray.kind).message.to_owned(),
        fix: format!("rm -r {}", quoted(stray.location.as_str())),
    }
}

/// Shell-quote `text` if needed for safe display in a suggested command.
fn quoted(text: &str) -> String {
    shlex::try_quote(text).map_or_else(|_| text.to_owned(), Cow::into_owned)
}

struct StrayProblem {
    code: &'static str,
    message: &'static str,
}

const fn stray_kind(kind: StrayKind) -> StrayProblem {
    match kind {
        StrayKind::NotAConversation => StrayProblem {
            code: "not_a_conversation",
            message: "directory is not a conversation",
        },
        StrayKind::TempFile => StrayProblem {
            code: "unused_temporary_file",
            message: "unused temporary file",
        },
        StrayKind::ImportStaging => StrayProblem {
            code: "unused_import_directory",
            message: "unused import directory",
        },
    }
}

/// Serialize one JSON record per finding in `report`.
pub(crate) fn render_json(report: &Report<'_>, pretty: bool) -> serde_json::Result<Vec<String>> {
    let mut records = vec![];
    for finding in &report.failures {
        records.push(load_failure::render_record(
            &finding.failure,
            finding.fix.map(Repair::name),
            pretty,
        )?);
    }

    for stray in report.strays {
        let problem = stray_kind(stray.kind);
        let record = StrayRecord {
            level: "warning",
            code: problem.code,
            message: problem.message,
            context: StrayContext {
                name: &stray.name,
                location: &stray.location,
            },
        };
        records.push(if pretty {
            serde_json::to_string_pretty(&record)?
        } else {
            serde_json::to_string(&record)?
        });
    }

    Ok(records)
}

#[derive(Serialize)]
struct StrayRecord<'a> {
    level: &'static str,
    code: &'static str,
    message: &'static str,
    context: StrayContext<'a>,
}

#[derive(Serialize)]
struct StrayContext<'a> {
    name: &'a str,
    location: &'a StorageLocation,
}

/// Confirmation callback taking the inspected conversation and whether more
/// conversations follow.
type Ask<'a> = dyn FnMut(&StoredConversation, bool) -> Result<Answer, cmd::Error> + 'a;

fn fix_prompt(repair: Repair) -> Prompt {
    Prompt {
        question: format!("Apply `{}` to this conversation?", repair.name()).into(),
        accept: "yes, apply it",
        accept_rest: "yes, and apply it to every remaining one",
        decline: "no, leave it as it is",
        decline_rest: "no, and leave every remaining one",
        caution: None,
    }
}

struct Fix {
    repair: Repair,
    ids: Vec<ConversationId>,
    confirm: ConfirmFlag,
}

impl Fix {
    /// Apply the repair to all target conversations, prompting via `ask` unless
    /// pre-confirmed (`--yes`).
    ///
    /// The confirmation prompt runs while holding each conversation's lock.
    /// Declined conversations are skipped; conversations that cannot be
    /// repaired are collected and reported as an error at the end of the run.
    fn apply(
        self,
        printer: &Printer,
        recovery: &dyn RecoveryBackend,
        diagnosis: &Diagnosis,
        ask: Option<&mut Ask<'_>>,
    ) -> Output {
        let name = self.repair.name();
        let ids = self.targets(diagnosis);
        if ids.is_empty() {
            printer.println(format!("Nothing to fix with `{name}`."));
            return Ok(());
        }

        let answers = self.confirm.answers();
        if ask.is_none() && answers.rest().is_none() {
            return Err("Cannot run `--fix` in a non-interactive environment.".into());
        }

        let tally = self.repair_each(recovery, &ids, answers, ask)?;
        tally.report(printer, name, ids.len())
    }

    fn repair_each(
        &self,
        recovery: &dyn RecoveryBackend,
        ids: &[ConversationId],
        mut answers: Answers,
        mut ask: Option<&mut Ask<'_>>,
    ) -> Result<Tally, cmd::Error> {
        let mut tally = Tally::default();
        for (index, id) in ids.iter().enumerate() {
            if answers.rest() == Some(false) {
                break;
            }

            let more = index + 1 < ids.len();
            let mut prompt_error = None;
            let outcome = recovery.repair(id, self.repair, &mut |stored| {
                answers
                    .decide(more, |more| match ask.as_mut() {
                        Some(ask) => ask(stored, more),
                        None => Ok(Answer::YesToAll),
                    })
                    .unwrap_or_else(|error| {
                        prompt_error = Some(error);
                        false
                    })
            })?;
            if let Some(error) = prompt_error {
                return Err(error);
            }

            tally.record(*id, outcome);
        }

        Ok(tally)
    }

    fn targets(&self, diagnosis: &Diagnosis) -> Vec<ConversationId> {
        let mut seen = HashSet::new();
        if !self.ids.is_empty() {
            return self
                .ids
                .iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .collect();
        }

        diagnosis
            .invalid
            .iter()
            .filter(|diagnosed| diagnosed.repair == Some(self.repair))
            .map(|diagnosed| diagnosed.conversation.id)
            .filter(|id| seen.insert(*id))
            .collect()
    }
}

/// Summary of a `--fix` run across target conversations.
#[derive(Debug, Default)]
struct Tally {
    applied: usize,
    failed: Vec<(ConversationId, RepairOutcome)>,
}

impl Tally {
    fn record(&mut self, id: ConversationId, outcome: RepairOutcome) {
        match outcome {
            RepairOutcome::Repaired => self.applied += 1,
            RepairOutcome::Declined => {}
            outcome => self.failed.push((id, outcome)),
        }
    }

    fn report(self, printer: &Printer, name: &str, count: usize) -> Output {
        if self.applied > 0 {
            printer.println(format!(
                "Applied `{name}` to {}.",
                counted(self.applied, "conversation", "conversations")
            ));
        } else if self.failed.is_empty() {
            printer.println("Nothing changed.");
        }
        if self.failed.is_empty() {
            return Ok(());
        }

        let entries = self
            .failed
            .iter()
            .map(|(id, outcome)| format!("{id}:  {}", why_not(*outcome)))
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "Failed to apply `{name}` to {} of {}:\n\n{entries}",
            self.failed.len(),
            counted(count, "conversation", "conversations")
        )
        .into())
    }
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

const fn why_not(outcome: RepairOutcome) -> &'static str {
    match outcome {
        RepairOutcome::Locked => "in use by another process",
        RepairOutcome::NotFound => "no conversation with this ID",
        RepairOutcome::NotApplicable => "the fix does not apply",
        RepairOutcome::Declined => "skipped",
        RepairOutcome::Repaired => "repaired",
    }
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
