//! How each option's value was written on the command line.
//!
//! clap reports `-t=read`, `-tread`, and `-t read` as the same value.
//! The difference matters to a flag whose value may instead be the first word
//! of the free text after it: only a value written as its own argument can be
//! that word.
//! [`Spelling::of`] walks the arguments with clap's own lexer and records, for
//! each occurrence of each option, whether its value was attached to it.
//!
//! clap also decides from a word's prefix whether it is a value or a flag: a
//! word starting with `-` is treated as a flag.
//! [`attach_from_end_values`] walks the arguments the same way and attaches any
//! value starting with a from-end position (`-3..-1`) to its preceding option
//! so clap parses it as that option's value.

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

/// Attach each value starting with a from-end position to the preceding option.
///
/// clap treats a word starting with `-` as a flag, unless the preceding option
/// accepts negative numbers and the entire word is a single negative number.
/// A range or list starting with a from-end position (`-3..-1`, `-3,-1`) is not
/// a single number, so clap rejects it as an unknown flag `-3`.
///
/// Because no `jp` flag starts with a digit, a word starting with `-` and a
/// digit directly after an option that takes a value is joined to that option
/// with `=`: `--reset -3..-1` becomes `--reset=-3..-1`.
/// Every other word is kept as written, including arguments after `--` and
/// arguments to a plugin command.
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

/// State of a pass over command-line arguments.
struct Walk<'a> {
    /// The innermost subcommand the walk has entered, whose options the next
    /// word is matched against.
    cmd: &'a Command,

    /// The option from the previous word, if it still awaits a separate value.
    open: Option<&'a Arg>,

    /// Whether all remaining words are positional values (after `--` or an
    /// external subcommand).
    values_only: bool,
}

impl<'a> Walk<'a> {
    const fn new(cmd: &'a Command) -> Self {
        Self {
            cmd,
            open: None,
            values_only: false,
        }
    }

    /// Advance past `token` and return `true` if it is a from-end value
    /// belonging to the preceding option.
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

    /// Advance past a non-option `word`: enter a declared subcommand, stop
    /// option parsing at a plugin subcommand, or ignore a positional value.
    fn enter(&mut self, word: &OsStr) {
        match self.cmd.find_subcommand(word) {
            Some(sub) => self.cmd = sub,
            // A plugin command parses its own arguments.
            None => self.values_only = self.cmd.is_allow_external_subcommands_set(),
        }
    }
}

/// Returns `true` if `word` starts with `-` followed by an ASCII digit.
fn starts_from_end(word: &OsStr) -> bool {
    matches!(word.as_encoded_bytes(), [b'-', digit, ..] if digit.is_ascii_digit())
}

/// Returns `true` if clap consumes `token` as the separate value of `option`.
///
/// clap consumes any non-flag word as the option's value, or any word at all
/// when the option sets `allow_hyphen_values`.
fn takes(option: &Arg, token: &ParsedArg<'_>) -> bool {
    option.is_allow_hyphen_values_set() || !(token.is_long() || token.is_short())
}

/// Classification of a command-line token against a command's declared options.
enum Written<'a> {
    /// An option that takes values, and whether a value is attached to it.
    Option { arg: &'a Arg, attached: bool },

    /// Flags that take no value, or an unrecognized option.
    Flags,

    /// A plain word (a value or subcommand name).
    Word,
}

impl<'a> Written<'a> {
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
