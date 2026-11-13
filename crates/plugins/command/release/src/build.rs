//! `jp release build`: building release artifacts via
//! `plugins.command.release.options.build` and storing them in workspace
//! user-local storage.
//!
//! [`build`] checks out the release's tagged commit (or the tip of its release
//! branch while the release is in preparation) in a temporary worktree, runs
//! the configured `build` command from the workspace root into a scratch
//! directory, validates the output with [`release_build::check`], and moves the
//! verified directory to `<user_workspace>/release/<version>/`.
//!
//! [`assess`] inspects a stored build's `manifest.json` for `jp release status`
//! to determine whether it was built with the `release` profile from the commit
//! named by `v<version>`.

use std::{fmt::Write as _, fs, process};

use camino::{Utf8Path, Utf8PathBuf};
use release_build::{MANIFEST, Manifest, Profile, check, read_manifest};
use semver::Version;

use crate::{
    options::BUILD_KEY,
    pipeline::{Pipeline, short},
    releases::{self, Releases},
    repository::{ORIGIN, Tags},
};

/// Build command contract appended to `jp release build --help`.
// Stored as a static string rather than a doc comment because clap reflows doc
// comment paragraphs onto single lines.
pub(crate) const CONTRACT: &str = r#"The build command:
  `plugins.command.release.options.build` runs from the workspace root, not
  from {source}, so an old release builds with today's scripts. It is split
  into words the way a shell splits a command, but no shell runs it: for pipes
  or variables, run a script. These are filled in first:

    {version}  The release's version, such as 0.3.0
    {source}   The workspace root, in a checkout of the commit being built
    {out}      An empty directory for what the build makes
    {profile}  `quick` with --quick, and `release` otherwise

  It has to name {source} and {out}.

What it leaves in {out}:
  - The files the release ships, such as archives or an installer, each for a
    target; a target may have several, such as an installer and an archive.
  - checksums.txt: each of those files' SHA-256 and no other's, in the format
    `sha256sum` and `shasum -a 256` write.
  - manifest.json: the version, the commit {source} is at, the profile, and
    each file with a target of the build's choosing, such as a Rust target:

      {
        "format": 1,
        "version": "0.3.0",
        "commit": "8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b",
        "profile": "release",
        "artifacts": [
          {
            "file": "app-aarch64-apple-darwin.tar.xz",
            "target": "aarch64-apple-darwin"
          }
        ]
      }

  Nothing else may be left there. A build that fails, or whose {out} breaks
  any of this, isn't kept, and every problem with it is named."#;

struct Target {
    commit: String,

    /// Ref name (`v<version>` or `release/<major>.<minor>`) displayed in status
    /// output.
    name: String,

    tagged: bool,
}

/// Temporary build output directory removed on drop unless persisted via
/// [`keep`].
struct Scratch {
    path: Utf8PathBuf,
    kept: bool,
}

impl Scratch {
    fn new(builds: &Utf8Path, version: &Version) -> Result<Self, String> {
        fs::create_dir_all(builds)
            .map_err(|error| format!("{builds} can't be created to keep builds in: {error}."))?;
        let path = builds.join(format!(".building-{version}-{}", process::id()));
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

/// Status of a stored release build in `<user_workspace>/release/<version>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kept {
    /// No build directory exists for the version.
    Nothing,

    /// Build uses the `release` profile and matches the tagged commit.
    Publishable,

    /// Build cannot be published (e.g. `"a quick build"` or a commit mismatch).
    ForTesting(String),
}

/// Build `version` with `profile`, streaming output lines to `each`, and store
/// the validated build under `builds/<version>/`.
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
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let (releases, tags) = repo.releases_and_tags()?;
    let target = target(&releases, &tags, version)?;
    if !repo.history.has_commit(&target.commit)? {
        repo.refs.fetch_tag(&releases::tag(version))?;
    }

    let worktree = repo.worktrees.add("build", &target.commit)?;
    let out = Scratch::new(builds, version)?;
    let (program, args) =
        command.command(version, &worktree.workspace(), &out.path, profile.name());

    each(&format!(
        "Building {version} from {} at {} with the {profile} profile.",
        target.name,
        short(&target.commit)
    ))?;
    let status = pipeline
        .shell
        .stream(&pipeline.root, &program, &args, each)
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

/// Resolve the commit to build for `version`: its tag `v<version>` if tagged,
/// or the tip of its release branch if in preparation.
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

/// Check whether `builds/<version>/` holds a publishable `release`-profile
/// build of commit `tagged`.
pub(crate) fn assess(builds: &Utf8Path, version: &Version, tagged: &str) -> Kept {
    let dir = builds.join(version.to_string());
    if !dir.is_dir() {
        return Kept::Nothing;
    }

    let Ok(manifest) = read_manifest(&dir) else {
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

/// Move `out` to `dir`, atomically replacing any existing directory at `dir`
/// and returning whether an existing build was replaced.
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
