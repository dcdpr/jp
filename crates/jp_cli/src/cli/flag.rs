//! Flags whose value is optional, and may be the first word of the free text
//! after it instead.
//!
//! `jp q --fork 2 hello` forks keeping two turns and asks "hello", while `jp q
//! --fork hello there` forks keeping every turn and asks "hello there".
//! [`OptionalValue`] declares such a flag and decides which of the two a word
//! is.
//! [`Flag`] reads the flag's values, and hands each word it gives up to the
//! command's [`FreeText`].
//!
//! A value attached to its flag (`--fork=2`, `-k2`) is always the flag's, and
//! so, by default, is one with no free text directly after it.
//! Only a value written as its own argument, directly before free text, goes
//! through [`OptionalValue::claim`].
//!
//! A flag decided by the word alone settles while the arguments are read.
//! One that needs the configuration is a field of a command's [`Stage`], which
//! holds it unsettled until the configuration is known.

use std::{error::Error, ffi::OsStr, fmt, io, marker::PhantomData, sync::Arc};

use clap::{Arg, ArgMatches, error::ErrorKind, value_parser};
use inquire::InquireError;
use jp_inquire::{InlineOption, prompt::PromptBackend};

use super::spelling::Spelling;
use crate::{cmd, shared::confirm::INTERRUPTED_EXIT_STATUS};

/// An error deciding what an argument means.
pub(crate) type SettleError = Box<dyn Error + Send + Sync>;

/// A flag whose value is optional.
///
/// Implementing [`parse`] alone gives a flag whose every value is its own;
/// [`claim`] is for a flag whose value may be free text instead.
///
/// [`claim`]: Self::claim
/// [`parse`]: Self::parse
pub(crate) trait OptionalValue {
    /// The clap argument id.
    const ID: &'static str;

    /// Whether every value written as its own argument goes through [`claim`].
    ///
    /// By default, only one written directly before free text does: with
    /// nothing after it, the word can only have been meant as the flag's value.
    ///
    /// [`claim`]: Self::claim
    const CLAIMS_EVERY_SEPARATE_VALUE: bool = false;

    /// One value of the flag.
    type Value: fmt::Debug;

    /// What deciding a word needs; `()` when the word alone decides.
    type Context: ?Sized;

    /// What the flag's values add up to for the command.
    type Output: fmt::Debug + Default;

    /// Declare the flag on `arg`.
    ///
    /// `arg` already reads an optional value as text, and an empty value where
    /// the flag is written without one.
    fn declare(arg: Arg) -> Arg;

    /// Parse a value that is the flag's.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not one the flag accepts.
    fn parse(value: &str) -> Result<Self::Value, clap::Error>;

    /// Decide whether a value written as its own argument is the flag's.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is the flag's and does not parse.
    fn claim(value: &str, _cx: &Self::Context) -> Result<Claim<Self::Value>, clap::Error> {
        Self::parse(value).map(Claim::Value)
    }

    /// Combine the flag's values, in command-line order.
    ///
    /// # Errors
    ///
    /// Returns an error when the values cannot be used together.
    fn collect(values: FlagValues<Self::Value>) -> Result<Self::Output, clap::Error>;
}

/// What a value written as its own argument turns out to be.
#[derive(Debug)]
pub(crate) enum Claim<T> {
    /// The flag's value.
    Value(T),

    /// The first word of the free text; the flag reads as written bare.
    Text,

    /// The first word of the free text, but only once the user agrees, because
    /// the flag written bare grants something.
    Confirm(Confirmation),
}

/// A question to put to the user before reading a value as free text.
#[derive(Debug)]
pub(crate) struct Confirmation {
    /// The question.
    ///
    /// Everything before its last line is shown above the choice.
    pub(crate) question: String,

    /// The label of the `y` choice, which says what agreeing does.
    pub(crate) accept: &'static str,

    /// The error when the user declines, or nobody is there to ask.
    pub(crate) refusal: String,
}

/// The free text a command reads after its flags.
pub(crate) trait FreeText: fmt::Debug + Default {
    /// The words, once every flag has given back what it does not claim.
    type Words: fmt::Debug + Default;

    /// Whether the command has free text that a flag can give a word to.
    fn takes_words(&self) -> bool;

    /// Whether the argument at clap index `index` is a word of free text.
    fn is_text(&self, index: usize) -> bool;

    /// Put `word` back at clap index `index`, where it was written.
    fn give(&mut self, index: usize, word: String);

    /// The words, in command-line order.
    fn into_words(self) -> Self::Words;
}

/// The free text of a command that has none.
#[derive(Debug, Default)]
pub(crate) struct NoText;

impl FreeText for NoText {
    type Words = ();

    fn takes_words(&self) -> bool {
        false
    }

    fn is_text(&self, _: usize) -> bool {
        false
    }

    fn give(&mut self, _: usize, _: String) {
        unreachable!("no value is read as free text where the command takes none");
    }

    fn into_words(self) {}
}

/// Each value of a flag, with its clap index.
///
/// `None` is the flag written bare, or with a value it gave up.
#[derive(Debug)]
pub(crate) struct FlagValues<V>(Vec<(usize, Option<V>)>);

impl<V> FlagValues<V> {
    /// The value the flag ends up with when written once at most.
    ///
    /// `None` without the flag, `Some(None)` written bare, and
    /// `Some(Some(value))` with a value.
    pub(crate) fn last(self) -> Option<Option<V>> {
        self.0.into_iter().last().map(|(_, value)| value)
    }

    /// Each value with its clap index, in command-line order.
    pub(crate) fn into_indexed(self) -> Vec<(usize, Option<V>)> {
        self.0
    }
}

impl<V> Default for FlagValues<V> {
    fn default() -> Self {
        Self(vec![])
    }
}

/// The argument `F` declares.
pub(crate) fn flag_arg<F: OptionalValue>() -> Arg {
    F::declare(
        Arg::new(F::ID)
            .num_args(0..=1)
            .default_missing_value("")
            .value_parser(value_parser!(String)),
    )
}

/// The values of an [`OptionalValue`] flag as written on the command line.
///
/// Every value that is the flag's for certain is parsed already; one that may
/// be free text waits for [`Settle::settle`].
pub(crate) struct Flag<F: OptionalValue> {
    values: Vec<(usize, Read<F::Value>)>,
    flag: PhantomData<F>,
}

/// A value as read, before the flag decides what any word is.
#[derive(Debug)]
enum Read<V> {
    Bare,
    Value(V),
    Undecided(String),
}

impl<F: OptionalValue> Flag<F> {
    /// Read the values of `F` from `matches`.
    ///
    /// # Errors
    ///
    /// Returns an error when a value that is the flag's does not parse.
    pub(crate) fn read(
        matches: &ArgMatches,
        spelling: &Spelling,
        text: &impl FreeText,
    ) -> Result<Self, clap::Error> {
        let Ok(Some(occurrences)) = matches.try_get_raw_occurrences(F::ID) else {
            return Ok(Self::default());
        };

        let occurrences: Vec<Vec<&OsStr>> = occurrences.map(Iterator::collect).collect();
        let attached = spelling.attached(F::ID, occurrences.len());
        let mut indices = matches.indices_of(F::ID).into_iter().flatten();

        let mut values = vec![];
        for (words, attached) in occurrences.into_iter().zip(attached) {
            // A value split out of a delimited list is never a single word of
            // free text.
            let separate = !attached && words.len() == 1;

            for word in words {
                let Some(index) = indices.next() else {
                    break;
                };
                let word = word
                    .to_str()
                    .ok_or_else(|| clap::Error::new(ErrorKind::InvalidUtf8))?;

                let claimable = separate
                    && text.takes_words()
                    && (F::CLAIMS_EVERY_SEPARATE_VALUE || text.is_text(index + 1));

                let read = if word.is_empty() {
                    Read::Bare
                } else if claimable {
                    Read::Undecided(word.to_owned())
                } else {
                    Read::Value(F::parse(word)?)
                };
                values.push((index, read));
            }
        }

        Ok(Self {
            values,
            flag: PhantomData,
        })
    }

    fn decide(
        self,
        cx: &F::Context,
        prompts: &ArgPrompts,
        text: &mut impl FreeText,
    ) -> Result<FlagValues<F::Value>, SettleError> {
        let mut values = Vec::with_capacity(self.values.len());
        for (index, read) in self.values {
            let value = match read {
                Read::Bare => None,
                Read::Value(value) => Some(value),
                Read::Undecided(word) => match F::claim(&word, cx)? {
                    Claim::Value(value) => Some(value),
                    Claim::Text => {
                        text.give(index, word);
                        None
                    }
                    Claim::Confirm(confirmation) => {
                        prompts.confirm(&confirmation)?;
                        text.give(index, word);
                        None
                    }
                },
            };
            values.push((index, value));
        }

        Ok(FlagValues(values))
    }
}

impl<F: OptionalValue<Context = ()>> Flag<F> {
    /// Read the values of `F` from `matches`, and decide each one by itself.
    ///
    /// A word the flag does not claim goes to `text`.
    ///
    /// # Errors
    ///
    /// Returns an error when a value that is the flag's does not parse, or the
    /// values cannot be used together.
    pub(crate) fn decide_now(
        matches: &ArgMatches,
        spelling: &Spelling,
        text: &mut impl FreeText,
    ) -> Result<F::Output, clap::Error> {
        // Nobody is asked while the arguments are read, so a flag decided by
        // the word alone has nothing to confirm.
        Self::read(matches, spelling, text)?
            .settle(&(), &ArgPrompts::default(), text)
            .map_err(|error| match error.downcast::<clap::Error>() {
                Ok(error) => *error,
                Err(error) => invalid_value(error),
            })
    }

    /// Read the values of `F` on a command without free text, where every value
    /// is the flag's.
    ///
    /// # Errors
    ///
    /// Returns an error when a value does not parse, or the values cannot be
    /// used together.
    pub(crate) fn strict(matches: &ArgMatches) -> Result<F::Output, clap::Error> {
        Self::decide_now(matches, &Spelling::default(), &mut NoText)
    }
}

impl<F: OptionalValue> Default for Flag<F> {
    fn default() -> Self {
        Self {
            values: vec![],
            flag: PhantomData,
        }
    }
}

impl<F: OptionalValue> fmt::Debug for Flag<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Flag")
            .field(&F::ID)
            .field(&self.values)
            .finish()
    }
}

/// Arguments whose meaning can only be decided once something more is known.
pub(crate) trait Settle: fmt::Debug + Default {
    /// What deciding needs.
    type Context: ?Sized;

    /// The decided arguments.
    type Output: fmt::Debug + Default;

    /// Decide what each argument is, handing each word read as free text to
    /// `text`.
    ///
    /// # Errors
    ///
    /// Returns an error when a word needs the user's confirmation and they
    /// decline, or nobody is there to ask, or when the decided values cannot be
    /// used together.
    fn settle(
        self,
        cx: &Self::Context,
        prompts: &ArgPrompts,
        text: &mut impl FreeText,
    ) -> Result<Self::Output, SettleError>;
}

impl<F: OptionalValue> Settle for Flag<F> {
    type Context = F::Context;
    type Output = F::Output;

    fn settle(
        self,
        cx: &F::Context,
        prompts: &ArgPrompts,
        text: &mut impl FreeText,
    ) -> Result<F::Output, SettleError> {
        let values = self.decide(cx, prompts, text)?;
        F::collect(values).map_err(Into::into)
    }
}

/// How far a command's arguments have been decided.
///
/// A command whose arguments need the configuration is generic over its stage:
/// [`Parsed`] while the configuration is resolved, [`Settled`] once it is.
pub(crate) trait Stage: fmt::Debug + Default {
    /// An argument decided against the configuration.
    type Config<T: Settle>: fmt::Debug + Default;

    /// The command's free text.
    type Text<T: FreeText>: fmt::Debug + Default;
}

/// Arguments read from the command line, with those that need the configuration
/// still undecided.
#[derive(Debug, Default)]
pub(crate) struct Parsed;

impl Stage for Parsed {
    type Config<T: Settle> = T;
    type Text<T: FreeText> = T;
}

/// Arguments with every meaning decided.
#[derive(Debug, Default)]
pub(crate) struct Settled;

impl Stage for Settled {
    type Config<T: Settle> = T::Output;
    type Text<T: FreeText> = T::Words;
}

/// Who answers when an argument's meaning needs the user's confirmation.
///
/// The default has nobody to ask, and refuses every question.
#[derive(Clone, Default)]
pub(crate) struct ArgPrompts(Option<Arc<dyn PromptBackend>>);

impl ArgPrompts {
    /// Ask `backend` whenever an argument needs confirming.
    pub(crate) fn new(backend: Arc<dyn PromptBackend>) -> Self {
        Self(Some(backend))
    }

    /// Ask the user `confirmation`'s question.
    ///
    /// Only `y` accepts.
    /// `n`, Esc, and having nobody to ask all refuse with the confirmation's
    /// refusal.
    /// Ctrl-C ends the run with a [`cmd::Error`] carrying the shell's interrupt
    /// status.
    ///
    /// # Errors
    ///
    /// Returns an error unless the user accepts.
    pub(crate) fn confirm(&self, confirmation: &Confirmation) -> Result<(), SettleError> {
        let refused = || confirmation.refusal.clone().into();

        let Some(backend) = self.0.as_deref() else {
            return Err(refused());
        };

        let options = vec![
            InlineOption::new('y', confirmation.accept),
            InlineOption::new('n', "no, stop here"),
        ];

        match backend.inline_select(
            &confirmation.question,
            options,
            Some('n'),
            &mut io::stderr(),
        ) {
            Ok('y') => Ok(()),
            Ok(_) | Err(InquireError::OperationCanceled) => Err(refused()),
            // `inquire` reads Ctrl-C as a keypress in raw mode, so no signal
            // stops the run; this reports the status the shell would have set.
            Err(InquireError::OperationInterrupted) => {
                Err(Box::new(cmd::Error::from(INTERRUPTED_EXIT_STATUS)))
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl fmt::Debug for ArgPrompts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.0.is_some() {
            "available"
        } else {
            "none"
        };
        f.debug_tuple("ArgPrompts").field(&state).finish()
    }
}

/// A clap error for an invalid value, reading `message`.
pub(crate) fn invalid_value(message: impl fmt::Display) -> clap::Error {
    clap::Error::raw(ErrorKind::InvalidValue, message.to_string())
}

#[cfg(test)]
#[path = "flag_tests.rs"]
mod tests;
