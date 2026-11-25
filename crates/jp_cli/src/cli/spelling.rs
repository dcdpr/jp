//! How each option's value was written on the command line.
//!
//! clap reports `-t=read`, `-tread`, and `-t read` as the same value.
//! The difference matters to a flag whose value may instead be the first word
//! of the free text after it: only a value written as its own argument can be
//! that word.
//! [`Spelling::of`] walks the arguments with clap's own lexer and records, for
//! each occurrence of each option, whether its value was attached to it.
//!
//! clap also decides from how a word is written whether it is a value at all: a
//! word that starts with `-` is a flag.
//! [`attach_from_end_values`] walks the arguments the same way, and attaches a
//! value that starts with a from-end position (`-3..-1`) to its option, so clap
//! reads it as that option's value.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
};

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

            let (arg, attached) = match Written::read(cmd, &token) {
                Written::Option { arg, attached } => (arg, attached),
                Written::Flags => continue,
                Written::Word => {
                    // A plain word is the next subcommand's name, the name of a
                    // plugin's command, or a positional value.
                    if let Some(sub) = next_subcommand(cmd, &token, expected.as_deref()) {
                        cmd = sub;
                        expected = path.next();
                    } else if expected
                        .as_deref()
                        .is_some_and(|name| token.to_value() == Ok(name))
                    {
                        // clap found a subcommand it does not declare: a
                        // plugin's command, whose arguments are the plugin's to
                        // read.
                        break;
                    }
                    continue;
                }
            };

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

/// Attach each value that starts with a from-end position to the option written
/// directly before it.
///
/// clap reads a word that starts with `-` as flags, unless the option before it
/// accepts a negative number and the whole word is one.
/// A range or list that starts with a from-end position (`-3..-1`, `-3,-1`) is
/// not a number as a whole, so clap rejects it as the unknown flag `-3`.
///
/// No flag of `jp` starts with a digit.
/// A word that starts with `-` and a digit, written directly after an option
/// that still takes a value, is therefore that value, and is joined to the
/// option with `=`: `--reset -3..-1` becomes `--reset=-3..-1`.
/// Every other word is kept as written, including each word after `--` and each
/// argument of a plugin's command.
///
/// `cmd` is the built command `argv` is matched against.
/// Returns `None` when no value is attached.
pub(crate) fn attach_from_end_values(cmd: &Command, argv: &[OsString]) -> Option<Vec<OsString>> {
    let raw = RawArgs::new(argv);
    let mut cursor = raw.cursor();

    let mut words = Vec::with_capacity(argv.len());
    words.extend(raw.next_os(&mut cursor).map(OsStr::to_owned));

    let mut walk = Walk::new(cmd);
    let mut attached = false;

    while let Some(token) = raw.next(&mut cursor) {
        let word = token.to_value_os();

        // The option a from-end value belongs to is the word before it.
        if walk.step(&token)
            && let Some(option) = words.last_mut()
        {
            option.push("=");
            option.push(word);
            attached = true;
        } else {
            words.push(word.to_owned());
        }
    }

    attached.then_some(words)
}

/// Where a walk over the words of a command line stands.
struct Walk<'a> {
    /// The innermost command the walk has descended into, whose options the
    /// next word is read against.
    cmd: &'a Command,

    /// The option the previous word wrote, while it still takes a value.
    open: Option<&'a Arg>,

    /// Whether every word from here on is a value, whatever it looks like.
    values_only: bool,
}

impl<'a> Walk<'a> {
    /// A walk that starts at the root command `cmd`, which must be built.
    const fn new(cmd: &'a Command) -> Self {
        Self {
            cmd,
            open: None,
            values_only: false,
        }
    }

    /// Step over `token`, the next word of the command line.
    ///
    /// Returns whether the word is a from-end value of the option written
    /// directly before it.
    fn step(&mut self, token: &ParsedArg<'_>) -> bool {
        let word = token.to_value_os();
        let option = self.open.take();

        // `--` ends the options of the command line.
        self.values_only |= token.is_escape();
        if self.values_only {
            return false;
        }

        if let Some(option) = option {
            if starts_from_end(word) {
                return true;
            }
            // The value of an option names no subcommand.
            if takes(option, token) {
                return false;
            }
        }

        match Written::read(self.cmd, token) {
            Written::Option {
                arg,
                attached: false,
            } => self.open = Some(arg),
            Written::Option { .. } | Written::Flags => {}
            Written::Word => self.enter(word),
        }

        false
    }

    /// Step over `word`, which starts no option: the name of a subcommand, the
    /// name of a plugin's command, or a positional value.
    fn enter(&mut self, word: &OsStr) {
        match self.cmd.find_subcommand(word) {
            Some(sub) => self.cmd = sub,
            // A plugin's command, whose arguments are the plugin's to read.
            None => self.values_only = self.cmd.is_allow_external_subcommands_set(),
        }
    }
}

/// Whether `word` starts with `-` and a digit, as a from-end position does.
fn starts_from_end(word: &OsStr) -> bool {
    matches!(word.as_encoded_bytes(), [b'-', digit, ..] if digit.is_ascii_digit())
}

/// Whether clap reads `token` as the value of `option`, written without one
/// directly before it.
///
/// clap gives an option any word that does not look like a flag, and any word
/// at all when the option accepts a value that does.
fn takes(option: &Arg, token: &ParsedArg<'_>) -> bool {
    option.is_allow_hyphen_values_set() || !(token.is_long() || token.is_short())
}

/// What one word of the command line writes, read against the options of a
/// command.
enum Written<'a> {
    /// An option that takes values, and whether a value is attached to it.
    Option { arg: &'a Arg, attached: bool },

    /// Flags that take no value, or an option the command does not declare.
    Flags,

    /// A word that starts no option: a value, or the name of a subcommand.
    Word,
}

impl<'a> Written<'a> {
    /// Read `token` against the options `cmd` declares.
    fn read(cmd: &'a Command, token: &ParsedArg<'_>) -> Self {
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
            return Self::Word;
        };

        match option {
            Some((arg, attached)) if takes_values(arg) => Self::Option { arg, attached },
            _ => Self::Flags,
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
