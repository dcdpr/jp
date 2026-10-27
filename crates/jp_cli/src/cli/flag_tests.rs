use std::{ffi::OsString, sync::Arc};

use clap::{ArgAction, Command, ValueEnum, builder::EnumValueParser};
use jp_inquire::prompt::MockPromptBackend;

use super::*;

/// How `--num` rejects `hello`: clap's own error for a value its parser
/// refused.
const NOT_A_NUMBER: &str = "error: invalid value 'hello' for '--num [<num>]': not a number\n\nFor \
                            more information, try '--help'.\n";

/// Parse a number for the test flags.
fn number(value: &str) -> Result<u32, String> {
    value.parse().map_err(|_| "not a number".to_owned())
}

/// `--num` / `-n`: a number, or the first word of the free text.
enum Num {}

impl OptionalValue for Num {
    const ID: &'static str = "num";

    type Context = ();
    type Output = FlagValues<u32>;
    type Value = u32;

    fn declare(arg: Arg) -> Arg {
        arg.short('n').long("num").action(ArgAction::Append)
    }

    fn parser() -> impl TypedValueParser<Value = u32> {
        number
    }

    fn claim(_: &str, parsed: Option<u32>, (): &()) -> Claim<u32> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<u32>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// The values of [`Mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeValue {
    Fast,
    Slow,
}

/// `--mode`: one of a fixed set of values, or the first word of the free text.
enum Mode {}

impl OptionalValue for Mode {
    const ID: &'static str = "mode";

    type Context = ();
    type Output = FlagValues<ModeValue>;
    type Value = ModeValue;

    fn declare(arg: Arg) -> Arg {
        arg.long("mode").help("The mode")
    }

    fn parser() -> impl TypedValueParser<Value = ModeValue> {
        EnumValueParser::<ModeValue>::new()
    }

    fn claim(_: &str, parsed: Option<ModeValue>, (): &()) -> Claim<ModeValue> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<ModeValue>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// `--strict`: a number, whose every value is its own.
enum Strict {}

impl OptionalValue for Strict {
    const ID: &'static str = "strict";

    type Context = ();
    type Output = FlagValues<u32>;
    type Value = u32;

    fn declare(arg: Arg) -> Arg {
        arg.long("strict")
    }

    fn parser() -> impl TypedValueParser<Value = u32> {
        number
    }

    fn collect(values: FlagValues<u32>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// `--any`: a number; any other word written on its own is free text, even with
/// nothing after it.
enum Any {}

impl OptionalValue for Any {
    const CLAIMS_EVERY_SEPARATE_VALUE: bool = true;
    const ID: &'static str = "any";

    type Context = ();
    type Output = FlagValues<u32>;
    type Value = u32;

    fn declare(arg: Arg) -> Arg {
        arg.long("any")
    }

    fn parser() -> impl TypedValueParser<Value = u32> {
        number
    }

    fn claim(_: &str, parsed: Option<u32>, (): &()) -> Claim<u32> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<u32>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// `--grant`: a number; written bare it grants something, so any other word is
/// free text only once the user agrees.
enum Grant {}

impl OptionalValue for Grant {
    const ID: &'static str = "grant";

    type Context = ();
    type Output = FlagValues<u32>;
    type Value = u32;

    fn declare(arg: Arg) -> Arg {
        arg.long("grant")
    }

    fn parser() -> impl TypedValueParser<Value = u32> {
        number
    }

    fn claim(word: &str, parsed: Option<u32>, (): &()) -> Claim<u32> {
        if let Some(value) = parsed {
            return Claim::Value(value);
        }

        Claim::Confirm(Confirmation {
            question: format!("start with `{word}`?"),
            accept: "yes",
            refusal: format!("`{word}` refused"),
        })
    }

    fn collect(values: FlagValues<u32>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// The positional words of the test command, written before `--` or after it.
#[derive(Debug, Default)]
struct Words {
    words: Vec<(usize, String)>,

    /// The indices of the words written before `--`.
    unescaped: Vec<usize>,
}

impl Words {
    fn read(matches: &ArgMatches) -> Self {
        let words_of = |id: &str| -> Vec<(usize, String)> {
            matches
                .indices_of(id)
                .into_iter()
                .flatten()
                .zip(
                    matches
                        .get_many::<String>(id)
                        .into_iter()
                        .flatten()
                        .cloned(),
                )
                .collect()
        };

        let mut words = words_of("words");
        let unescaped = words.iter().map(|(index, _)| *index).collect();
        words.extend(words_of("escaped"));

        Self { words, unescaped }
    }
}

impl FreeText for Words {
    type Words = Vec<String>;

    fn takes_words(&self) -> bool {
        true
    }

    fn is_text(&self, index: usize) -> bool {
        self.unescaped.contains(&index)
    }

    fn give(&mut self, index: usize, word: String) {
        self.words.push((index, word));
    }

    fn into_words(mut self) -> Vec<String> {
        self.words.sort_by_key(|(index, _)| *index);
        self.words.into_iter().map(|(_, word)| word).collect()
    }
}

/// Match `argv` (after the program name) against a command declaring every test
/// flag, a flag `-x`, and positional words before and after `--`.
fn matched(argv: &[&str]) -> (ArgMatches, Spelling) {
    let argv: Vec<OsString> = ["prog"].iter().chain(argv).map(OsString::from).collect();

    let mut cmd = Command::new("prog")
        .arg(flag_arg::<Num>())
        .arg(flag_arg::<Strict>())
        .arg(flag_arg::<Any>())
        .arg(flag_arg::<Grant>())
        .arg(flag_arg::<Mode>())
        .arg(Arg::new("x").short('x').action(ArgAction::SetTrue))
        .arg(Arg::new("words").action(ArgAction::Append).num_args(1..))
        .arg(
            Arg::new("escaped")
                .action(ArgAction::Append)
                .num_args(1..)
                .last(true),
        );

    let matches = cmd.try_get_matches_from_mut(&argv).unwrap();
    cmd.build();
    let spelling = Spelling::of(&cmd, &matches, &argv);
    (matches, spelling)
}

/// Decide `F` on `argv`: each of its values, and the free text after it.
fn decide<F>(argv: &[&str]) -> Result<(Vec<Option<u32>>, Vec<String>), String>
where
    F: OptionalValue<Context = (), Output = FlagValues<u32>>,
{
    settle::<F>(argv, &ArgPrompts::default())
}

/// [`decide`] with `prompts` to answer a confirmation.
fn settle<F>(argv: &[&str], prompts: &ArgPrompts) -> Result<(Vec<Option<u32>>, Vec<String>), String>
where
    F: OptionalValue<Context = (), Output = FlagValues<u32>>,
{
    let (matches, spelling) = matched(argv);
    let mut words = Words::read(&matches);

    let values = Flag::<F>::read(&matches, &spelling, &words)
        .map_err(|error| error.to_string())?
        .settle(&(), prompts, &mut words)
        .map_err(|error| error.to_string())?;

    let values = values
        .into_indexed()
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    Ok((values, words.into_words()))
}

fn answering(answer: char) -> ArgPrompts {
    ArgPrompts::new(Arc::new(
        MockPromptBackend::new().with_inline_responses([answer]),
    ))
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

#[test]
fn a_word_before_free_text_that_the_flag_claims_is_its_value() {
    assert_eq!(
        decide::<Num>(&["--num", "2", "hello"]),
        Ok((vec![Some(2)], words(&["hello"])))
    );
}

#[test]
fn a_word_before_free_text_that_the_flag_does_not_claim_is_free_text() {
    assert_eq!(
        decide::<Num>(&["--num", "hello", "world"]),
        Ok((vec![None], words(&["hello", "world"])))
    );
}

#[test]
fn a_word_given_up_keeps_its_place_in_the_free_text() {
    assert_eq!(
        decide::<Num>(&["before", "-n", "hello", "after"]),
        Ok((vec![None], words(&["before", "hello", "after"])))
    );
}

// `=`, or a value sharing the short flag's argument, says the value is the
// flag's, so one that does not parse is an error rather than free text.
#[test]
fn an_attached_value_is_always_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num=hello", "world"]),
        Err(NOT_A_NUMBER.to_owned())
    );
    assert_eq!(
        decide::<Num>(&["-nhello", "world"]),
        Err(NOT_A_NUMBER.to_owned())
    );
    assert_eq!(
        decide::<Num>(&["-n=hello", "world"]),
        Err(NOT_A_NUMBER.to_owned())
    );
}

// With no free text after it, the word can only be meant as the flag's value.
#[test]
fn a_word_with_nothing_after_it_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello"]),
        Err(NOT_A_NUMBER.to_owned())
    );
}

#[test]
fn a_word_before_another_option_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello", "-x", "world"]),
        Err(NOT_A_NUMBER.to_owned())
    );
}

// Words after `--` are free text, but a value directly before `--` is not
// directly before them.
#[test]
fn a_word_before_a_double_dash_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello", "--", "world"]),
        Err(NOT_A_NUMBER.to_owned())
    );
}

#[test]
fn a_bare_flag_has_no_value() {
    assert_eq!(
        decide::<Num>(&["hello", "--num"]),
        Ok((vec![None], words(&["hello"])))
    );
}

#[test]
fn a_flag_that_claims_nothing_keeps_every_word() {
    assert_eq!(
        decide::<Strict>(&["--strict", "hello", "world"]),
        Err(
            "error: invalid value 'hello' for '--strict [<strict>]': not a number\n\nFor more \
             information, try '--help'.\n"
                .to_owned()
        )
    );
    assert_eq!(
        decide::<Strict>(&["--strict", "3", "world"]),
        Ok((vec![Some(3)], words(&["world"])))
    );
}

#[test]
fn a_flag_claiming_every_separate_word_gives_up_a_last_word() {
    assert_eq!(
        decide::<Any>(&["--any", "hello"]),
        Ok((vec![None], words(&["hello"])))
    );
    assert_eq!(
        decide::<Any>(&["--any", "hello", "-x"]),
        Ok((vec![None], words(&["hello"])))
    );
}

#[test]
fn a_word_needing_confirmation_is_refused_with_nobody_to_ask() {
    assert_eq!(
        decide::<Grant>(&["--grant", "hello", "world"]),
        Err("`hello` refused".to_owned())
    );
}

#[test]
fn a_word_needing_confirmation_is_free_text_once_confirmed() {
    assert_eq!(
        settle::<Grant>(&["--grant", "hello", "world"], &answering('y')),
        Ok((vec![None], words(&["hello", "world"])))
    );
}

#[test]
fn a_word_needing_confirmation_is_refused_when_declined() {
    assert_eq!(
        settle::<Grant>(&["--grant", "hello", "world"], &answering('n')),
        Err("`hello` refused".to_owned())
    );
}

/// Decide `--mode` on `argv`: each of its values, and the free text after it.
fn decide_mode(argv: &[&str]) -> Result<(Vec<Option<ModeValue>>, Vec<String>), String> {
    let (matches, spelling) = matched(argv);
    let mut words = Words::read(&matches);

    let values = Flag::<Mode>::read(&matches, &spelling, &words)
        .map_err(|error| error.to_string())?
        .settle(&(), &ArgPrompts::default(), &mut words)
        .map_err(|error| error.to_string())?;

    let values = values
        .into_indexed()
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    Ok((values, words.into_words()))
}

// The flag's own parser stays in charge of its values, so help lists what an
// enum flag accepts, as for any clap argument.
#[test]
fn help_lists_the_values_of_an_enum_flag() {
    let mut cmd = Command::new("prog").arg(flag_arg::<Mode>());

    assert_eq!(
        cmd.render_help().to_string(),
        "Usage: prog [OPTIONS]\n\nOptions:\n      --mode [<mode>]  The mode [possible values: \
         fast, slow]\n  -h, --help           Print help\n"
    );
}

#[test]
fn an_enum_flag_parses_its_value_into_the_enum() {
    assert_eq!(
        decide_mode(&["--mode", "slow", "hello"]),
        Ok((vec![Some(ModeValue::Slow)], words(&["hello"])))
    );
}

#[test]
fn an_enum_flag_gives_up_a_word_that_is_none_of_its_values() {
    assert_eq!(
        decide_mode(&["--mode", "what", "now"]),
        Ok((vec![None], words(&["what", "now"])))
    );
}

// The error is the enum parser's own, so it lists the values the flag takes.
#[test]
fn an_attached_value_an_enum_flag_rejects_lists_its_values() {
    assert_eq!(
        decide_mode(&["--mode=medium", "now"]),
        Err(
            "error: invalid value 'medium' for '--mode [<mode>]'\n  [possible values: fast, \
             slow]\n\nFor more information, try '--help'.\n"
                .to_owned()
        )
    );
}

// A command without free text has nowhere to put a word, so every value is
// the flag's, wherever it sits.
#[test]
fn a_command_without_free_text_keeps_every_word_for_the_flag() {
    let (matches, _) = matched(&["--num", "hello", "world"]);

    let error = Flag::<Num>::strict(&matches).unwrap_err();

    assert_eq!(error.to_string(), NOT_A_NUMBER);
}
