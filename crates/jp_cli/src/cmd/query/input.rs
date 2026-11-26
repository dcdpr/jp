//! Reading the text a query argument stands for: an `@path` file, piped stdin,
//! or a `--schema` value.

use std::{
    env, fs,
    io::{self, IsTerminal},
};

use camino::Utf8PathBuf;
use jp_config::fs::expand_tilde;

use crate::{
    PATH_STRING_PREFIX,
    error::{Error, Result},
    schema::parse_schema_dsl,
};

/// Parse a schema string as either a concise DSL or raw JSON Schema.
#[expect(clippy::needless_pass_by_value)]
pub(super) fn parse_schema(s: String) -> Result<schemars::Schema> {
    parse_schema_dsl(&s)
        .map_err(|e| Error::Schema(e.to_string()))?
        .try_into()
        .map_err(Into::into)
}

/// Read the piped stdin payload, or the empty string when stdin is a terminal.
///
/// The payload is taken verbatim: interior newlines (fenced code blocks,
/// paragraph breaks, ...) are part of the message.
/// Only the final line terminator is dropped, so request composition can add
/// its own separators.
pub(super) fn read_piped_stdin() -> Result<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }

    let mut piped = io::read_to_string(stdin.lock())?;
    if piped.ends_with('\n') {
        piped.pop();
        if piped.ends_with('\r') {
            piped.pop();
        }
    }

    Ok(piped)
}

/// Resolve an argument value that may use the `@path` form.
///
/// A value starting with `@` is replaced by the contents of the file it names.
/// Any other value is returned as-is.
pub(super) fn string_or_path(s: &str) -> Result<String> {
    match arg_file_path(s) {
        Some(path) => read_arg_file(path),
        None => Ok(s.to_owned()),
    }
}

/// Path an argument value refers to, if it uses the `@path` form.
///
/// Returns `None` for a value without the sigil, and for a bare `@` whose
/// remainder is empty or only whitespace — that is ordinary text, not a path.
fn arg_file_path(s: &str) -> Option<&str> {
    s.strip_prefix(PATH_STRING_PREFIX)
        .filter(|path| !path.trim().is_empty())
}

/// Path a positional query refers to, if it uses the `@path` form.
///
/// Only a query consisting of exactly one value participates, so an `@` word
/// inside a multi-word query stays ordinary text.
pub(super) fn query_file_path(values: &[String]) -> Option<&str> {
    match values {
        [value] => arg_file_path(value),
        _ => None,
    }
}

/// Read the file an `@path` argument names, expanding a leading `~` to `$HOME`.
pub(super) fn read_arg_file(path: &str) -> Result<String> {
    let path = expand_tilde(path, env::var("HOME").ok()).unwrap_or_else(|| Utf8PathBuf::from(path));

    fs::read_to_string(&path).map_err(|source| Error::ArgFile {
        path: path.into_string(),
        source,
    })
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
