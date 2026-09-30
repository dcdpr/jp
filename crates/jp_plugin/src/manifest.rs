//! The plugin manifest: a line of JSON embedded in a plugin binary.
//!
//! The host reads a plugin's manifest from the file, without running it, to
//! learn where the plugin attaches to `jp` before deciding whether to run it.
//! [`find`] is the host's side: it scans a file's bytes for the manifest line.
//! [`manifest!`] is the plugin's side: it builds the line as a string constant.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Plugin Manifest".
//!
//! [`manifest!`]: crate::manifest!

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the manifest line's framing this crate reads and writes.
///
/// Changes only when the framing changes, or a field is removed or changes
/// meaning.
/// New claims are added as optional keys, or behind a higher
/// [`Manifest::protocol`], and leave it alone.
pub const MANIFEST_VERSION: u32 = 1;

/// The most bytes of JSON a manifest may hold.
pub const MAX_MANIFEST_SIZE: usize = 64 * 1024;

/// Where a plugin attaches to `jp`, and what it needs from the host.
///
/// Read from the binary before the host has decided to run it, and repeated in
/// the plugin's answer to `describe`.
/// Keys a host does not know are ignored, so a newer plugin's optional claims
/// do not stop an older host from reading the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The lowest host protocol version the plugin needs.
    pub protocol: u32,

    /// One line saying what the plugin does.
    pub description: String,

    /// The command path the plugin claims: `["serve", "web"]` for `jp serve
    /// web`.
    pub command: Vec<String>,
}

/// Why a file's manifest cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// The manifest line appears more than once, and the copies could disagree.
    #[error("the manifest appears more than once in the file")]
    Duplicate,

    /// The manifest is framed in a version this host does not read.
    #[error("the manifest is format version {0}; this plugin was built for a newer `jp`")]
    UnsupportedVersion(u32),

    /// No line ending within [`MAX_MANIFEST_SIZE`] bytes.
    #[error("the manifest is longer than {MAX_MANIFEST_SIZE} bytes")]
    TooLarge,

    /// The manifest's bytes are not UTF-8.
    #[error("the manifest is not valid UTF-8")]
    NotUtf8,

    /// The manifest is not the JSON a manifest has to be.
    #[error("the manifest is not valid: {0}")]
    Invalid(String),

    /// A string in the manifest holds a control character other than a newline
    /// or a tab.
    #[error("the manifest holds a control character")]
    ControlCharacter,
}

impl Manifest {
    /// Read the manifest out of a single manifest line.
    ///
    /// For a plugin reading back the constant [`manifest!`] built: everything
    /// up to the first space is taken to be the marker, and the rest is
    /// validated the way [`find`] validates what it finds.
    ///
    /// # Errors
    ///
    /// Returns the [`ManifestError`] that makes the JSON unusable.
    ///
    /// [`manifest!`]: crate::manifest!
    pub fn from_line(line: &str) -> Result<Self, ManifestError> {
        let (_, json) = line
            .split_once(' ')
            .ok_or_else(|| ManifestError::Invalid("no JSON after the marker".to_owned()))?;

        parse(json.trim_end().as_bytes())
    }

    /// Check the fields the way a manifest read from a file is checked.
    ///
    /// For manifest fields that arrived another way, such as a plugin's
    /// `describe` answer: they reach the terminal the same way.
    ///
    /// # Errors
    ///
    /// Returns the [`ManifestError`] that makes the fields unusable.
    pub fn check(&self) -> Result<(), ManifestError> {
        let value =
            serde_json::to_value(self).map_err(|e| ManifestError::Invalid(e.to_string()))?;

        if has_control_character(&value) {
            return Err(ManifestError::ControlCharacter);
        }

        validate(self)
    }
}

/// Build a plugin's manifest line as a string constant.
///
/// ```ignore
/// static MANIFEST: &str = jp_plugin::manifest!(
///     protocol: 1,
///     description: "Print JP directory paths",
///     command: ["path"],
/// );
/// ```
///
/// The description and command segments must be plain string literals: they are
/// written into the JSON as they appear in the source, so a raw string or an
/// escape JSON does not share (such as `\'`) produces a manifest the host
/// rejects.
/// Read the constant back with [`DescribeResponse::from_manifest`], which both
/// checks it and keeps it in the binary.
///
/// [`DescribeResponse::from_manifest`]: crate::message::DescribeResponse::from_manifest
#[macro_export]
macro_rules! manifest {
    (
        protocol: $protocol:literal,
        description: $description:literal,
        command: [$($segment:literal),+ $(,)?] $(,)?
    ) => {
        concat!(
            "jp-plugin/v1 {\"protocol\":",
            stringify!($protocol),
            ",\"description\":",
            stringify!($description),
            ",\"command\":",
            stringify!([$($segment),+]),
            "}\n",
        )
    };
}

/// Find the manifest in the bytes of a plugin file.
///
/// The manifest is the marker `jp-plugin/v<N>`, a space, and a JSON object, up
/// to the first newline or NUL byte or the end of the file.
/// It may sit anywhere: in a string constant of a compiled binary, or in a
/// comment of a script.
///
/// Returns `Ok(None)` for a file that carries no manifest.
///
/// # Errors
///
/// Returns the [`ManifestError`] that makes the file's manifest unusable.
pub fn find(bytes: &[u8]) -> Result<Option<Manifest>, ManifestError> {
    let mut found = None;
    let mut from = 0;

    while let Some(offset) = find_marker(&bytes[from..]) {
        let start = from + offset;
        from = start + 1;

        let Some((version, json_start)) = read_marker(&bytes[start..]) else {
            continue;
        };

        if found.is_some() {
            return Err(ManifestError::Duplicate);
        }

        found = Some((version, start + json_start));
    }

    let Some((version, json_start)) = found else {
        return Ok(None);
    };

    if version != MANIFEST_VERSION {
        return Err(ManifestError::UnsupportedVersion(version));
    }

    let rest = &bytes[json_start..];
    let end = rest
        .iter()
        .take(MAX_MANIFEST_SIZE + 1)
        .position(|&b| b == b'\n' || b == 0);

    let json = match end {
        Some(end) => &rest[..end],
        None if rest.len() <= MAX_MANIFEST_SIZE => rest,
        None => return Err(ManifestError::TooLarge),
    };

    if json.len() > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge);
    }

    parse(json).map(Some)
}

/// The byte offset of the next `jp-plugin/v` in `bytes`.
///
/// Skips from one `j` to the next rather than comparing at every offset: a
/// compiled binary is megabytes long, and the prefix almost never occurs.
fn find_marker(bytes: &[u8]) -> Option<usize> {
    const PREFIX: &[u8] = b"jp-plugin/v";

    let mut from = 0;
    while let Some(offset) = bytes[from..].iter().position(|&b| b == PREFIX[0]) {
        let at = from + offset;
        if bytes[at..].starts_with(PREFIX) {
            return Some(at);
        }
        from = at + 1;
    }

    None
}

/// Read the version and the offset of the JSON from a candidate marker.
///
/// A candidate is a marker only when the prefix is followed by digits, a space,
/// and the opening brace of the JSON; anything else is a coincidence in the
/// file, such as the prefix appearing inside the host's own code.
fn read_marker(candidate: &[u8]) -> Option<(u32, usize)> {
    let after_prefix = &candidate[b"jp-plugin/v".len()..];
    let digits = after_prefix
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();

    if digits == 0 || after_prefix.get(digits..digits + 2) != Some(&b" {"[..]) {
        return None;
    }

    // Too many digits to be a version this host knows, which is what an
    // overflow would have to mean.
    let version = std::str::from_utf8(&after_prefix[..digits])
        .ok()?
        .parse()
        .unwrap_or(u32::MAX);

    Some((version, b"jp-plugin/v".len() + digits + 1))
}

/// Parse and validate the JSON of a manifest.
fn parse(json: &[u8]) -> Result<Manifest, ManifestError> {
    if json.len() > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge);
    }

    let json = std::str::from_utf8(json).map_err(|_| ManifestError::NotUtf8)?;
    let value: Value =
        serde_json::from_str(json).map_err(|e| ManifestError::Invalid(e.to_string()))?;

    if has_control_character(&value) {
        return Err(ManifestError::ControlCharacter);
    }

    let manifest: Manifest =
        serde_json::from_value(value).map_err(|e| ManifestError::Invalid(e.to_string()))?;

    validate(&manifest)?;
    Ok(manifest)
}

/// Whether any string in `value`, key or value, holds a control character other
/// than `\n` and `\t`.
///
/// The description reaches the terminal from a binary nobody has approved, and
/// an escape sequence in it would reach the terminal with it.
fn has_control_character(value: &Value) -> bool {
    let bad = |s: &str| s.chars().any(|c| c.is_control() && c != '\n' && c != '\t');

    match value {
        Value::String(s) => bad(s),
        Value::Array(items) => items.iter().any(has_control_character),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| bad(key) || has_control_character(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// Check what serde cannot: that the fields hold usable values.
fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.description.trim().is_empty() {
        return Err(ManifestError::Invalid("`description` is empty".to_owned()));
    }

    if manifest.command.is_empty() {
        return Err(ManifestError::Invalid("`command` is empty".to_owned()));
    }

    for segment in &manifest.command {
        if segment.is_empty() || segment.starts_with('-') || segment.contains(char::is_whitespace) {
            return Err(ManifestError::Invalid(format!(
                "`{segment}` in `command` is not a subcommand name"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
