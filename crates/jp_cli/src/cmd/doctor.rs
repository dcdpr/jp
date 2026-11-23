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

use std::{borrow::Cow, collections::HashSet, str::FromStr as _};

use clap::{Arg, ArgAction, ArgMatches, Command, builder::PossibleValuesParser};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::ConversationId;
use jp_inquire::{InlineOption, InlineSelect};
use jp_printer::Printer;
use jp_storage::{
    backend::{Diagnosis, RecoveryBackend, Repair, RepairOutcome, StrayEntry, StrayKind},
    validate::StorageLocation,
};
use jp_term::width::display_width;
use serde::Serialize;
use tracing::warn;

use crate::{
    cli::CommandArgs,
    cmd::{self, Output},
    format::no_persist_note,
    load_failure::{self, LoadFailure, RecordExtras},
    shared::confirm::{ConfirmFlag, decide},
};

/// The exit status of a report that lists a problem.
///
/// Distinct from a run that failed (1) and from a mistyped command (2, which
/// clap uses).
const PROBLEMS_FOUND_EXIT_STATUS: u8 = 3;

/// `jp doctor`.
#[derive(Debug)]
pub(crate) struct Doctor {
    /// The fix `--fix` names.
    fix: Option<Repair>,

    /// The conversations `--id` names, to apply the fix to; every conversation
    /// the fix covers when empty.
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
    /// Report what `recovery` finds, or apply the fix `--fix` names.
    ///
    /// A report that lists a problem ends the run with
    /// [`PROBLEMS_FOUND_EXIT_STATUS`].
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
            interactive,
        };
        let result = fix.apply(printer, recovery, &diagnosis);
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

    /// What the conversation store holds beside its conversations.
    pub strays: &'a [StrayEntry],
}

/// A stored conversation that does not load, and the fix that makes it load.
pub(crate) struct Finding<'a> {
    pub failure: LoadFailure<'a>,
    pub fix: Option<Repair>,

    /// Whether the conversation is in the archive.
    pub archived: bool,
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
                    archived: diagnosed.conversation.archived,
                })
                .collect(),
            strays: &diagnosis.strays,
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

/// How serious a problem is, which colours its row.
#[derive(Clone, Copy)]
enum Severity {
    /// Something the user's data is missing or cannot be read.
    Error,

    /// Something the store holds that it does not need.
    Warning,
}

/// Orange, in the 256-colour palette.
const WARNING_COLOR: Color = Color::AnsiValue(208);

/// One problem, as a row of the report's table.
struct Row {
    severity: Severity,

    /// What the problem is about.
    target: String,

    /// What is wrong with it.
    problem: String,

    /// The command that deals with it.
    fix: String,
}

/// Render the text report: a table of problems under the section they belong
/// to, then the commands that apply a fix to many conversations at once.
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

/// A line for each fix that covers more than one conversation, with the command
/// that applies it to all of them at once.
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

/// Lay `rows` out in aligned columns, one bullet per row.
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

/// A row per conversation that does not load, live or `archived`, by its first
/// copy's failure.
///
/// An archived conversation is out of reach of the fixes and of `jp c rm`, so
/// its row suggests unarchiving it.
fn conversation_rows<'a>(
    findings: &'a [Finding<'_>],
    archived: bool,
) -> impl Iterator<Item = Row> + 'a {
    let mut seen = HashSet::new();
    findings
        .iter()
        .filter(move |finding| finding.archived == archived)
        .filter(move |finding| seen.insert(finding.failure.id))
        .map(|finding| {
            let id = finding.failure.id;
            let fix = match finding.fix {
                _ if finding.archived => format!("jp c unarchive {id}"),
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

/// `text` quoted for a POSIX shell when it needs quoting.
fn quoted(text: &str) -> String {
    shlex::try_quote(text).map_or_else(|_| text.to_owned(), Cow::into_owned)
}

/// The fixed fields of a stray entry's problem.
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

/// Render one JSON record per problem.
///
/// A conversation that does not load gets the load-failure report's record,
/// with the fix that makes it load.
/// With `pretty`, each record is indented across several lines.
pub(crate) fn render_json(report: &Report<'_>, pretty: bool) -> serde_json::Result<Vec<String>> {
    let mut records = vec![];
    for finding in &report.failures {
        let extras = RecordExtras {
            fix: finding.fix.map(Repair::name),
            archived: finding.archived,
        };
        records.push(load_failure::render_record(
            &finding.failure,
            extras,
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

/// A stray entry's problem, as a JSON record.
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

/// A run of `--fix`: the repair, and what to apply it to.
struct Fix {
    repair: Repair,

    /// The conversations `--id` names; every conversation the diagnosis says
    /// the repair covers when empty.
    ids: Vec<ConversationId>,

    confirm: ConfirmFlag,
    interactive: bool,
}

impl Fix {
    /// Apply the repair to every conversation it is for, after one confirmation
    /// for all of them.
    ///
    /// A conversation the repair cannot change is skipped, and the run ends
    /// with an error naming each one.
    fn apply(
        self,
        printer: &Printer,
        recovery: &dyn RecoveryBackend,
        diagnosis: &Diagnosis,
    ) -> Output {
        let name = self.repair.name();
        let ids = self.targets(diagnosis);
        if ids.is_empty() {
            printer.println(format!("Nothing to fix with `{name}`."));
            return Ok(());
        }

        let subject = counted(ids.len(), "conversation", "conversations");
        if self.confirm.preference() != Some(false) && !self.ask(printer, &subject)? {
            printer.println("Nothing changed.");
            return Ok(());
        }

        let mut failed = vec![];
        for id in &ids {
            match recovery.repair(id, self.repair, &mut |_| true)? {
                RepairOutcome::Repaired => {}
                outcome => failed.push((*id, outcome)),
            }
        }

        let applied = ids.len() - failed.len();
        if applied > 0 {
            printer.println(format!(
                "Applied `{name}` to {}.",
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
            "Failed to apply `{name}` to {} of {subject}:\n\n{entries}",
            failed.len()
        )
        .into())
    }

    /// The conversations to apply the repair to, each once.
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

    /// Ask whether to apply the repair to `subject`, the conversations it is
    /// for.
    ///
    /// Fails when nobody can answer: writing to the user's conversations is
    /// never assumed.
    fn ask(&self, printer: &Printer, subject: &str) -> Result<bool, cmd::Error> {
        if !self.interactive {
            return Err("Cannot run `--fix` in a non-interactive environment.".into());
        }

        let options = vec![
            InlineOption::new('y', "yes, apply it"),
            InlineOption::new('n', "no, change nothing"),
        ];
        let question = format!("Apply `{}` to {subject}?", self.repair.name());
        let select = InlineSelect::new(question, options).with_default('n');

        Ok(decide(select.prompt(&mut printer.prompt_writer()))?)
    }
}

/// `count` and the noun for it, e.g. `1 conversation` or `2 conversations`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Why a repair left a conversation unchanged.
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
