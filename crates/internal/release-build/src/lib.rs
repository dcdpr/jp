//! Validation for release build output directories (`manifest.json`,
//! `checksums.txt`, and artifact files).
//!
//! `jp release build` validates the output directory produced by a project's
//! build command with [`check`] before persisting the build.
//! Publishers load a stored build's manifest with [`read_manifest`], verify its
//! metadata against the release being published, and re-verify artifact
//! checksums with [`check_files`] before uploading.
//!
//! See `docs/rfd/122-release-pipeline.md` ("Building a release").

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    fs::{self, File},
    io::{ErrorKind, Read as _},
};

use camino::Utf8Path;
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// Subdirectory under workspace user-local storage where release builds are
/// stored (`<user_workspace>/release/<version>/`).
pub const BUILDS: &str = "release";

/// Build manifest filename inside a build directory.
pub const MANIFEST: &str = "manifest.json";

/// SHA-256 checksum filename inside a build directory.
pub const CHECKSUMS: &str = "checksums.txt";

/// Schema version of [`Manifest`] supported by this crate.
pub const FORMAT: u64 = 1;

/// Build profile passed to the release build command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Production release build.
    Release,

    /// Faster unoptimized build for testing the release pipeline.
    Quick,
}

impl Profile {
    /// Profile identifier passed to the build command and recorded in
    /// `manifest.json`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Quick => "quick",
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Parsed `manifest.json` describing the artifacts produced by a release build.
///
/// Unknown JSON fields are ignored for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub artifacts: Vec<Artifact>,
}

/// An artifact file recorded in [`Manifest`] and its target triple.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub file: String,
    pub target: String,
}

/// Validate the build output in `out` against `version`, `commit`, and
/// `profile`, and return the parsed [`Manifest`].
///
/// Verifies that `manifest.json` matches the requested build parameters and
/// that all artifacts and checksums pass [`check_files`].
///
/// # Errors
///
/// Returns every validation problem found as a list of human-readable messages.
pub fn check(
    out: &Utf8Path,
    version: &Version,
    commit: &str,
    profile: Profile,
) -> Result<Manifest, Vec<String>> {
    let manifest = read_manifest(out).map_err(|problem| vec![problem])?;
    let mut problems = vec![];

    if manifest.version != version.to_string() {
        problems.push(format!(
            "{MANIFEST} names version {}, but the build was of {version}.",
            manifest.version
        ));
    }
    if manifest.commit != commit {
        problems.push(format!(
            "{MANIFEST} names commit `{}`, but the build was of {commit}.",
            manifest.commit
        ));
    }
    if manifest.profile != profile.name() {
        problems.push(format!(
            "{MANIFEST} names the {} profile, but the build was asked for the {profile} profile.",
            manifest.profile
        ));
    }
    problems.extend(check_files(out, &manifest));

    if problems.is_empty() {
        Ok(manifest)
    } else {
        Err(problems)
    }
}

/// Validate the artifact files and `checksums.txt` in `dir` against `manifest`.
///
/// Requires at least one artifact with a valid file name and non-empty target,
/// verifies that each listed file exists in `dir` and matches its SHA-256 entry
/// in `checksums.txt`, and rejects any unlisted files in `dir`.
#[must_use]
pub fn check_files(dir: &Utf8Path, manifest: &Manifest) -> Vec<String> {
    let mut problems = vec![];
    if manifest.artifacts.is_empty() {
        problems.push(format!("{MANIFEST} lists no archives."));
    }

    let mut files = BTreeSet::new();
    for artifact in &manifest.artifacts {
        let file = artifact.file.as_str();
        if !is_file_name(file) {
            problems.push(format!(
                "{MANIFEST} lists `{file}`, which can't be the name of an archive in the build's \
                 directory."
            ));
            continue;
        }
        if !files.insert(file) {
            problems.push(format!("{MANIFEST} lists `{file}` twice."));
            continue;
        }

        if artifact.target.trim().is_empty() {
            problems.push(format!("{MANIFEST} names no target for `{file}`."));
        }
        if !dir.join(file).is_file() {
            problems.push(format!(
                "{MANIFEST} lists `{file}`, which the build didn't write."
            ));
        }
    }

    problems.extend(check_checksums(dir, &files));
    problems.extend(strays(dir, &files));

    problems
}

/// Read and parse `<dir>/manifest.json`.
///
/// # Errors
///
/// Returns an error if `manifest.json` is missing, malformed JSON, has an
/// unsupported `format` version, or is missing required fields.
pub fn read_manifest(dir: &Utf8Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(dir.join(MANIFEST)).map_err(|error| match error.kind() {
        ErrorKind::NotFound => format!("The build wrote no {MANIFEST}."),
        _ => format!("{MANIFEST} can't be read: {error}."),
    })?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{MANIFEST} isn't JSON: {error}."))?;

    // Check `format` before deserializing so newer schema versions produce a
    // version-mismatch error rather than a missing-field error.
    match value.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(format) => {
            return Err(format!(
                "{MANIFEST} is format {format}, and this plugin reads format {FORMAT}."
            ));
        }
        None => return Err(format!("{MANIFEST} names no format.")),
    }

    serde_json::from_value(value).map_err(|error| format!("{MANIFEST} can't be read: {error}."))
}

/// Return whether `name` is a plain file name (not a path or reserved metadata
/// file) inside the build directory.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\'])
        && !matches!(name, "." | ".." | MANIFEST | CHECKSUMS)
}

fn check_checksums(out: &Utf8Path, files: &BTreeSet<&str>) -> Vec<String> {
    let Ok(text) = fs::read_to_string(out.join(CHECKSUMS)) else {
        return vec![format!("The build wrote no {CHECKSUMS}.")];
    };

    let mut problems = vec![];
    let mut listed = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((hash, file)) = parse_checksum(line) else {
            problems.push(format!(
                "{CHECKSUMS} line {} isn't a SHA-256 and a file name, as `sha256sum` writes them.",
                index + 1
            ));
            continue;
        };
        if !files.contains(file) {
            problems.push(format!(
                "{CHECKSUMS} lists `{file}`, which {MANIFEST} doesn't."
            ));
            continue;
        }
        if listed.insert(file, hash).is_some() {
            problems.push(format!("{CHECKSUMS} lists `{file}` twice."));
        }
    }

    for file in files {
        let Some(expected) = listed.get(file) else {
            problems.push(format!("{CHECKSUMS} has no SHA-256 for `{file}`."));
            continue;
        };
        let path = out.join(file);
        if !path.is_file() {
            continue;
        }
        match sha256(&path) {
            Ok(actual) if actual == *expected => {}
            Ok(actual) => problems.push(format!(
                "{CHECKSUMS} gives `{file}` the SHA-256 {expected}, but its contents hash to \
                 {actual}."
            )),
            Err(problem) => problems.push(problem),
        }
    }

    problems
}

const SHA256_HEX_DIGITS: usize = 64;
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Parse a `sha256sum`-formatted line (`<64-hex-digits>  <name>` or
/// `<64-hex-digits> *<name>`), returning the lowercase hex digest and file
/// name.
fn parse_checksum(line: &str) -> Option<(String, &str)> {
    let (hash, rest) = line.split_at_checked(SHA256_HEX_DIGITS)?;
    if !hash.chars().all(|char| char.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.strip_prefix(' ')?;
    let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;

    (!name.is_empty()).then(|| (hash.to_ascii_lowercase(), name))
}

/// Report any entries in `out` that are not in `files`, `manifest.json`, or
/// `checksums.txt`.
fn strays(out: &Utf8Path, files: &BTreeSet<&str>) -> Vec<String> {
    let entries = match out.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) => return vec![format!("{out} can't be read: {error}.")],
    };

    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_owned())
        .filter(|name| !files.contains(name.as_str()) && name != MANIFEST && name != CHECKSUMS)
        .collect();
    names.sort();

    names
        .into_iter()
        .map(|name| format!("The build left `{name}`, which {MANIFEST} doesn't list."))
        .collect()
}

/// Compute the lowercase hex SHA-256 digest of the file at `path`.
fn sha256(path: &Utf8Path) -> Result<String, String> {
    let unreadable = |error| format!("{path} can't be read: {error}.");
    let mut file = File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; READ_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(unreadable)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(SHA256_HEX_DIGITS), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
