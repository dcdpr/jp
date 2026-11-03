//! How each option's value was written on the command line.
//!
//! clap reports `-t=read`, `-tread`, and `-t read` as the same value.
//! The difference matters to a flag whose value may instead be the first word
//! of the free text after it: only a value written as its own argument can be
//! that word.
//! [`Spelling::of`] walks the arguments with clap's own lexer and records, for
//! each occurrence of each option, whether its value was attached to it.

use std::{collections::HashMap, ffi::OsString};

use clap::{Arg, ArgMatches, Command};
use clap_lex::{ParsedArg, RawArgs};

/// Whether each occurrence of an option had its value attached to it.
///
/// The default records nothing, which reads every value as attached.
#[derive(Debug, Default)]
pub(crate) struct Spelling {
    attached: HashMap<String, Vec<bool>>,
}

impl Spelling {
    /// Record how `argv` spelled each option.
    ///
    /// `cmd` is the built command `argv` was matched against, and `matches` the
    /// result; together they say which subcommands the walk descends into.
    pub(crate) fn of(cmd: &Command, matches: &ArgMatches, argv: &[OsString]) -> Self {
        let raw = RawArgs::new(argv);
        let mut cursor = raw.cursor();
        raw.next_os(&mut cursor);

        let mut path = subcommand_names(matches).into_iter();
        let mut expected = path.next();
        let mut cmd = cmd;
        let mut spelling = Self::default();

        while let Some(token) = raw.next(&mut cursor) {
            // Everything after `--` is a value, whatever it looks like.
            if token.is_escape() {
                break;
            }

            let option = if let Some((name, value)) = token.to_long() {
                name.ok()
                    .and_then(|name| find_long(cmd, name))
                    .map(|arg| (arg, value.is_some()))
            } else if let Some(mut shorts) = token.to_short() {
                // The first short flag in a cluster that takes a value ends the
                // cluster: the rest of it, if any, is that value.
                let mut option = None;
                while let Some(Ok(short)) = shorts.next_flag() {
                    let Some(arg) = find_short(cmd, short) else {
                        break;
                    };
                    if takes_values(arg) {
                        option = Some((arg, shorts.next_value_os().is_some()));
                        break;
                    }
                }
                option
            } else {
                // A plain word is the next subcommand's name, the name of a
                // plugin's command, or a positional value.
                if let Some(sub) = next_subcommand(cmd, &token, expected.as_deref()) {
                    cmd = sub;
                    expected = path.next();
                } else if expected
                    .as_deref()
                    .is_some_and(|name| token.to_value() == Ok(name))
                {
                    // clap found a subcommand it does not declare: a plugin's
                    // command, whose arguments are the plugin's to read.
                    break;
                }
                continue;
            };

            let Some((arg, attached)) = option else {
                continue;
            };
            if !takes_values(arg) {
                continue;
            }

            spelling
                .attached
                .entry(arg.get_id().to_string())
                .or_default()
                .push(attached);

            if !attached
                && let Some(next) = raw.peek(&cursor)
                && consumes(arg, &next, cmd, expected.as_deref())
            {
                raw.next_os(&mut cursor);
            }
        }

        spelling
    }

    /// Whether each occurrence of the option `id` had its value attached, given
    /// that clap counted `occurrences` of them.
    ///
    /// When the walk counted a different number, every occurrence reads as
    /// attached: an attached value is always the option's own, so a value the
    /// walk could not place is never given away.
    pub(crate) fn attached(&self, id: &str, occurrences: usize) -> Vec<bool> {
        match self.attached.get(id) {
            Some(found) if found.len() == occurrences => found.clone(),
            _ => vec![true; occurrences],
        }
    }
}

/// The names of the subcommands `matches` selected, outermost first.
fn subcommand_names(matches: &ArgMatches) -> Vec<String> {
    let mut names = vec![];
    let mut matches = matches;
    while let Some((name, sub)) = matches.subcommand() {
        names.push(name.to_owned());
        matches = sub;
    }

    names
}

fn find_long<'a>(cmd: &'a Command, name: &str) -> Option<&'a Arg> {
    cmd.get_arguments().find(|arg| {
        arg.get_long() == Some(name)
            || arg
                .get_all_aliases()
                .is_some_and(|aliases| aliases.contains(&name))
    })
}

fn find_short(cmd: &Command, short: char) -> Option<&Arg> {
    cmd.get_arguments().find(|arg| {
        arg.get_short() == Some(short)
            || arg
                .get_all_short_aliases()
                .is_some_and(|aliases| aliases.contains(&short))
    })
}

fn takes_values(arg: &Arg) -> bool {
    arg.get_action().takes_values()
}

/// Whether clap read `next` as the value of `arg`, written without one
/// attached.
///
/// An option that requires a value always takes the next argument.
/// One whose value is optional takes it unless it starts another option, is
/// `--`, or names the subcommand that comes next.
fn consumes(arg: &Arg, next: &ParsedArg<'_>, cmd: &Command, expected: Option<&str>) -> bool {
    if arg
        .get_num_args()
        .is_none_or(|range| range.min_values() > 0)
    {
        return true;
    }

    if next.is_escape() || next.is_long() || next.is_short() {
        return false;
    }

    next_subcommand(cmd, next, expected).is_none()
}

/// The subcommand of `cmd` that `token` names, when it is the one clap went on
/// to match.
fn next_subcommand<'a>(
    cmd: &'a Command,
    token: &ParsedArg<'_>,
    expected: Option<&str>,
) -> Option<&'a Command> {
    let expected = expected?;
    cmd.find_subcommand(token.to_value_os())
        .filter(|sub| sub.get_name() == expected)
}

#[cfg(test)]
#[path = "spelling_tests.rs"]
mod tests;
