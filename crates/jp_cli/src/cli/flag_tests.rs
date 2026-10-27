use std::{ffi::OsString, sync::Arc};

use clap::{ArgAction, Command};
use jp_inquire::prompt::MockPromptBackend;

use super::*;

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

    fn parse(value: &str) -> Result<u32, clap::Error> {
        value
            .parse()
            .map_err(|_| invalid_value(format!("not a number: '{value}'")))
    }

    fn claim(value: &str, (): &()) -> Result<Claim<u32>, clap::Error> {
        Ok(value.parse().map_or(Claim::Text, Claim::Value))
    }

    fn collect(values: FlagValues<u32>) -> Result<Self::Output, clap::Error> {
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

    fn parse(value: &str) -> Result<u32, clap::Error> {
        Num::parse(value)
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

    fn parse(value: &str) -> Result<u32, clap::Error> {
        Num::parse(value)
    }

    fn claim(value: &str, cx: &()) -> Result<Claim<u32>, clap::Error> {
        Num::claim(value, cx)
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

    fn parse(value: &str) -> Result<u32, clap::Error> {
        Num::parse(value)
    }

    fn claim(value: &str, (): &()) -> Result<Claim<u32>, clap::Error> {
        if let Ok(value) = value.parse() {
            return Ok(Claim::Value(value));
        }

        Ok(Claim::Confirm(Confirmation {
            question: format!("start with `{value}`?"),
            accept: "yes",
            refusal: format!("`{value}` refused"),
        }))
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
        Err("error: not a number: 'hello'".to_owned())
    );
    assert_eq!(
        decide::<Num>(&["-nhello", "world"]),
        Err("error: not a number: 'hello'".to_owned())
    );
    assert_eq!(
        decide::<Num>(&["-n=hello", "world"]),
        Err("error: not a number: 'hello'".to_owned())
    );
}

// With no free text after it, the word can only be meant as the flag's value.
#[test]
fn a_word_with_nothing_after_it_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello"]),
        Err("error: not a number: 'hello'".to_owned())
    );
}

#[test]
fn a_word_before_another_option_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello", "-x", "world"]),
        Err("error: not a number: 'hello'".to_owned())
    );
}

// Words after `--` are free text, but a value directly before `--` is not
// directly before them.
#[test]
fn a_word_before_a_double_dash_is_the_flags() {
    assert_eq!(
        decide::<Num>(&["--num", "hello", "--", "world"]),
        Err("error: not a number: 'hello'".to_owned())
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
        Err("error: not a number: 'hello'".to_owned())
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

// A command without free text has nowhere to put a word, so every value is
// the flag's, wherever it sits.
#[test]
fn a_command_without_free_text_keeps_every_word_for_the_flag() {
    let (matches, _) = matched(&["--num", "hello", "world"]);

    let error = Flag::<Num>::strict(&matches).unwrap_err();

    assert_eq!(error.to_string(), "error: not a number: 'hello'");
}
