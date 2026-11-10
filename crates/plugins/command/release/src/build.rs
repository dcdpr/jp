//! `jp release build`: a release's binaries, made by the project's `build`
//! command and kept in the workspace's user-local storage.
//!
//! [`build`] checks out the commit the release's tag names, or the tip of its
//! release branch while it is in preparation, in a worktree of its own, and
//! runs `build` from the workspace root to write the archives, `checksums.txt`,
//! and `manifest.json` into an empty directory.
//! [`check`] holds what the command left there to what was asked for, and only
//! then is it moved to `release/<version>/`, replacing an earlier build of that
//! version.
//!
//! A kept build is publishable when it was made with the `release` profile from
//! the commit the release's tag names, which [`assess`] reads from its manifest
//! for `status`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    fs::{self, File},
    io::{ErrorKind, Read as _},
    process,
};

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    options::BUILD_KEY,
    pipeline::{Pipeline, short},
    releases::{self, Releases},
    repository::{ORIGIN, Tags},
};

/// The directory below the workspace's user-local storage builds are kept in,
/// one directory per version.
pub(crate) const BUILDS: &str = "release";

/// The file a build lists what it made in.
const MANIFEST: &str = "manifest.json";

/// The file a build gives each archive's SHA-256 in.
const CHECKSUMS: &str = "checksums.txt";

/// The version of [`Manifest`] this plugin reads.
const FORMAT: u64 = 1;

/// How a release is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    /// The binaries a release ships.
    Release,

    /// Binaries made faster, to test a build with.
    Quick,
}

impl Profile {
    /// The profile's name, as `{profile}` passes it and a manifest names it.
    pub(crate) fn name(self) -> &'static str {
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
/// Fields this plugin doesn't read are ignored, so a newer build command can
/// add some.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Manifest {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub artifacts: Vec<Artifact>,
}

/// An archive a build made, and the platform its binaries run on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Artifact {
    pub file: String,
    pub target: String,
}

/// What a build is of.
struct Target {
    /// The commit built.
    commit: String,

    /// What names the commit: the release's tag, or while the release is in
    /// preparation, its release branch.
    name: String,

    /// Whether the release's tag names the commit.
    tagged: bool,
}

/// The empty directory a build writes into, removed when dropped unless kept.
struct Scratch {
    path: Utf8PathBuf,
    kept: bool,
}

impl Scratch {
    /// A new, empty directory in `builds` for a build of `version`.
    fn new(builds: &Utf8Path, version: &Version) -> Result<Self, String> {
        fs::create_dir_all(builds)
            .map_err(|error| format!("{builds} can't be created to keep builds in: {error}."))?;
        let path = builds.join(format!(".building-{version}-{}", process::id()));
        // Left by an earlier run that had this process's id and was stopped.
        if path.exists() {
            fs::remove_dir_all(&path)
                .map_err(|error| format!("{path} can't be emptied for the build: {error}."))?;
        }
        fs::create_dir(&path)
            .map_err(|error| format!("{path} can't be created for the build: {error}."))?;

        Ok(Self { path, kept: false })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.kept {
            drop(fs::remove_dir_all(&self.path));
        }
    }
}

/// How a build of a release kept in user-local storage stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kept {
    /// No build of it is kept.
    Nothing,

    /// A build publishers take.
    Publishable,

    /// A build publishers refuse, with what it is, such as `a quick build`.
    ForTesting(String),
}

/// Build `version` with `profile`, handing each line the build prints to
/// `each`, and keep what it made in `builds`.
///
/// Returns what was built and where it is kept.
/// The error says why `version` can't be built, why the build failed, or what
/// it left that can't be kept; nothing is kept then.
pub(crate) fn build(
    pipeline: &Pipeline<'_>,
    builds: &Utf8Path,
    version: &Version,
    profile: Profile,
    each: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<String, String> {
    let command = pipeline.options.build.as_ref().ok_or_else(|| {
        format!("`{BUILD_KEY}` isn't set, so there is no command to build {version} with.")
    })?;
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let target = target(&repo.releases()?, &repo.tags()?, version)?;
    // A tag only `origin` has can name a commit no branch here holds.
    if !repo.has_commit(&target.commit)? {
        repo.fetch_tag(&releases::tag(version))?;
    }

    let worktree = repo.worktree("build", &target.commit)?;
    let out = Scratch::new(builds, version)?;
    let (program, args) =
        command.command(version, &worktree.workspace(), &out.path, profile.name());

    each(&format!(
        "Building {version} from {} at {} with the {profile} profile.",
        target.name,
        short(&target.commit)
    ))?;
    let status = repo
        .shell()
        .stream(repo.root(), &program, &args, each)
        .map_err(|error| format!("`{BUILD_KEY}` couldn't build {version}: {error}"))?;
    if !status.success() {
        return Err(format!(
            "`{BUILD_KEY}` failed building {version}: {status}."
        ));
    }

    let manifest = check(&out.path, version, &target.commit, profile).map_err(|problems| {
        format!(
            "The build of {version} left what `jp release` can't accept, so nothing was kept:\n{}",
            problems
                .iter()
                .map(|problem| format!("- {problem}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;
    let dir = builds.join(version.to_string());
    let replaced = keep(out, &dir)?;

    Ok(built(version, &target, profile, &manifest, &dir, replaced))
}

/// What `version` is built from: the commit its tag names, or while it is in
/// preparation, the tip of its release branch.
///
/// The error says there is nothing of `version` to build, or that its tag names
/// one commit here and another on `origin`.
fn target(releases: &Releases, tags: &Tags, version: &Version) -> Result<Target, String> {
    let tag = releases::tag(version);
    if releases.tagged.contains(version) {
        let commit = match (tags.local.get(&tag), tags.origin.get(&tag)) {
            (Some(here), Some(there)) if here != there => {
                return Err(format!(
                    "{tag} names {} here, but {} on {ORIGIN}; delete the one that is wrong before \
                     building {version}.",
                    short(here),
                    short(there)
                ));
            }
            (Some(commit), _) | (None, Some(commit)) => commit.clone(),
            (None, None) => return Err(format!("{tag} names no commit git can read.")),
        };

        return Ok(Target {
            commit,
            name: tag,
            tagged: true,
        });
    }

    let Some(line) = releases.line(version) else {
        return Err(format!(
            "There is no {}, so {version} hasn't been cut.",
            releases::branch(version)
        ));
    };
    if releases.in_preparation(line) != Some(version) {
        return Err(format!(
            "{version} is neither tagged nor in preparation on {}, so there is nothing of it to \
             build.",
            line.branch
        ));
    }

    Ok(Target {
        commit: line.tip.clone(),
        name: line.branch.clone(),
        tagged: false,
    })
}

/// Check what a build of `version` from `commit` with `profile` left in `out`,
/// and return its manifest.
///
/// `manifest.json` has to name what was asked for and list at least one
/// archive, each a file in `out` for a platform no other archive is for;
/// `checksums.txt`, in `sha256sum`'s format, has to give each archive the
/// SHA-256 of its contents, and nothing else may be in `out`.
/// The error lists every problem found, one sentence each.
pub(crate) fn check(
    out: &Utf8Path,
    version: &Version,
    commit: &str,
    profile: Profile,
) -> Result<Manifest, Vec<String>> {
    let manifest = read_manifest(&out.join(MANIFEST)).map_err(|problem| vec![problem])?;
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
    if manifest.artifacts.is_empty() {
        problems.push(format!("{MANIFEST} lists no archives."));
    }

    let mut files = BTreeSet::new();
    let mut targets = BTreeMap::new();
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

        let target = artifact.target.trim();
        if target.is_empty() {
            problems.push(format!("{MANIFEST} names no target for `{file}`."));
        } else if let Some(other) = targets.insert(target, file) {
            problems.push(format!(
                "{MANIFEST} lists two archives for {target}: `{other}` and `{file}`."
            ));
        }
        if !out.join(file).is_file() {
            problems.push(format!(
                "{MANIFEST} lists `{file}`, which the build didn't write."
            ));
        }
    }

    problems.extend(check_checksums(out, &files));
    problems.extend(strays(out, &files));

    if problems.is_empty() {
        Ok(manifest)
    } else {
        Err(problems)
    }
}

/// How the build of `version` kept in `builds` stands, when the release's tag
/// names `tagged`.
///
/// Reads only the build's manifest: the archives are hashed when a build is
/// made, and again by whatever publishes them.
pub(crate) fn assess(builds: &Utf8Path, version: &Version, tagged: &str) -> Kept {
    let dir = builds.join(version.to_string());
    if !dir.is_dir() {
        return Kept::Nothing;
    }

    let Ok(manifest) = read_manifest(&dir.join(MANIFEST)) else {
        return Kept::ForTesting(format!("a build without a {MANIFEST} it can read"));
    };
    if manifest.version != version.to_string() {
        return Kept::ForTesting(format!("a build of {}", manifest.version));
    }
    if manifest.profile != Profile::Release.name() {
        return Kept::ForTesting(format!("a {} build", manifest.profile));
    }
    if manifest.commit != tagged {
        return Kept::ForTesting(format!(
            "a build of {}, which {} doesn't name",
            short(&manifest.commit),
            releases::tag(version)
        ));
    }

    Kept::Publishable
}

/// Read the manifest at `path`.
///
/// The error says it is missing, isn't JSON, is a format this plugin doesn't
/// read, or lacks what it has to name.
fn read_manifest(path: &Utf8Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(path).map_err(|error| match error.kind() {
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
                "{MANIFEST} is format {format}, and this jp-release reads format {FORMAT}."
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

/// Move the build in `out` to `dir`, replacing what is there, and return
/// whether it replaced an earlier build.
///
/// The error says the build can't be moved; an earlier build is put back then.
fn keep(mut out: Scratch, dir: &Utf8Path) -> Result<bool, String> {
    let replaced = dir.exists();
    let earlier = dir.with_file_name(format!(
        ".replaced-{}-{}",
        dir.file_name().unwrap_or_default(),
        process::id()
    ));
    if replaced {
        fs::rename(dir, &earlier)
            .map_err(|error| format!("The build kept in {dir} can't be replaced: {error}."))?;
    }

    if let Err(error) = fs::rename(&out.path, dir) {
        if replaced {
            drop(fs::rename(&earlier, dir));
        }
        return Err(format!("The build can't be kept in {dir}: {error}."));
    }
    out.kept = true;
    if replaced {
        drop(fs::remove_dir_all(&earlier));
    }

    Ok(replaced)
}

/// What to tell the user about the build of `version` from `target` with
/// `profile` that `manifest` lists, now kept in `dir`.
fn built(
    version: &Version,
    target: &Target,
    profile: Profile,
    manifest: &Manifest,
    dir: &Utf8Path,
    replaced: bool,
) -> String {
    let width = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.file.chars().count())
        .max()
        .unwrap_or_default();
    let listing = manifest
        .artifacts
        .iter()
        .fold(String::new(), |mut listing, artifact| {
            let _ = writeln!(listing, "    {:width$}  {}", artifact.file, artifact.target);
            listing
        });

    let mut paragraphs = vec![format!(
        "Built {version} from {} at {} with the {profile} profile, into {dir}:\n\n{listing}",
        target.name,
        short(&target.commit)
    )];
    if replaced {
        paragraphs.push(format!(
            "It replaces the build of {version} kept there before.\n"
        ));
    }
    match (profile, target.tagged) {
        (Profile::Quick, _) => {
            paragraphs.push("It's a quick build, for testing: publishers refuse it.\n".to_owned());
        }
        (Profile::Release, false) => paragraphs.push(format!(
            "{version} isn't tagged yet, so publishers refuse this build until {} names {}.\n",
            releases::tag(version),
            short(&target.commit)
        )),
        (Profile::Release, true) => {}
    }

    paragraphs.join("\n")
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
