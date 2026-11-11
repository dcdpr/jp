//! A release's build: the files a project's build command made for one release,
//! kept in a directory with `checksums.txt` and `manifest.json`.
//!
//! `jp release build` holds what a build command left to this format with
//! [`check`] before keeping it.
//! A publisher reads a kept build's manifest with [`read_manifest`], compares
//! what it names with what it publishes, and hashes the files again with
//! [`check_files`], so it publishes nothing that changed since it was built.
//!
//! See: `docs/rfd/122-release-pipeline.md`, "Building a release"

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

/// The directory below a workspace's user-local storage builds are kept in, one
/// directory per version.
pub const BUILDS: &str = "release";

/// The file a build lists what it made in.
pub const MANIFEST: &str = "manifest.json";

/// The file a build gives each file's SHA-256 in.
pub const CHECKSUMS: &str = "checksums.txt";

/// The version of [`Manifest`] this crate reads.
pub const FORMAT: u64 = 1;

/// How a release is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The binaries a release ships.
    Release,

    /// Binaries made faster, to test a build with.
    Quick,
}

impl Profile {
    /// The profile's name, as a build command is told it and a manifest names
    /// it.
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

/// What a build made, as its `manifest.json` lists it.
///
/// Fields this crate doesn't read are ignored, so a newer build command can add
/// some.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub artifacts: Vec<Artifact>,
}

/// A file a build made, and the platform it is for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub file: String,
    pub target: String,
}

/// Check what a build of `version` from `commit` with `profile` left in `out`,
/// and return its manifest.
///
/// `manifest.json` has to name what was asked for, and its files have to pass
/// [`check_files`].
/// The error lists every problem found, one sentence each.
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

/// What is wrong with the files of the build in `dir`, which `manifest` lists.
///
/// `manifest` has to list at least one file, each in `dir` and each with the
/// platform it is for; `checksums.txt`, in `sha256sum`'s format, has to give
/// each the SHA-256 of its contents, and nothing else may be in `dir`.
/// Every problem found is listed, one sentence each.
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

/// Read the manifest of the build in `dir`.
///
/// The error says it is missing, isn't JSON, is a format this crate doesn't
/// read, or lacks what it has to name.
pub fn read_manifest(dir: &Utf8Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(dir.join(MANIFEST)).map_err(|error| match error.kind() {
        ErrorKind::NotFound => format!("The build wrote no {MANIFEST}."),
        _ => format!("{MANIFEST} can't be read: {error}."),
    })?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{MANIFEST} isn't JSON: {error}."))?;

    // Read before the rest, which a newer format may lay out differently.
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

/// Whether `name` can be an archive's: a file directly in the build's
/// directory, and neither of the files that describe the build.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\'])
        && !matches!(name, "." | ".." | MANIFEST | CHECKSUMS)
}

/// What is wrong with `checksums.txt` in `out` for the archives `files`.
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
        // A missing archive is a problem of the manifest's.
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

/// The SHA-256 and file name of a line of `checksums.txt`, as `sha256sum`
/// writes them: the hash, a space, then a space or `*`, then the name.
///
/// The hash is returned in lowercase.
fn parse_checksum(line: &str) -> Option<(String, &str)> {
    let (hash, rest) = line.split_at_checked(64)?;
    if !hash.chars().all(|char| char.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.strip_prefix(' ')?;
    let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;

    (!name.is_empty()).then(|| (hash.to_ascii_lowercase(), name))
}

/// What is in `out` besides the archives `files` and the files describing them.
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

/// The SHA-256 of the contents of the file at `path`, in lowercase hex.
///
/// The error says the file can't be read.
fn sha256(path: &Utf8Path) -> Result<String, String> {
    let unreadable = |error| format!("{path} can't be read: {error}.");
    let mut file = File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
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
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
