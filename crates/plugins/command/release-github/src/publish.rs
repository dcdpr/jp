//! Publishing a release to GitHub: uploading build artifacts and configured
//! workspace assets to the GitHub Release for `v<version>`.
//!
//! New releases are created as drafts, populated with assets, and then
//! published so they never appear without their files.
//! Existing releases are updated in place: the title, description, and any
//! matching asset filenames are replaced while other attached files are
//! preserved.
//! The highest published semantic version is explicitly marked as `latest` so
//! publishing a patch on an older release line does not steal the `latest`
//! badge.

use std::{collections::BTreeSet, fmt::Write as _, fs};

use camino::Utf8Path;
use jp_github::{
    Error, Octocrab,
    models::releases::Release,
    params::{MakeLatest, NewRelease, ReleaseUpdate},
};
use release_build::{CHECKSUMS, MANIFEST, Profile, check_files, read_manifest};
use semver::Version;

use crate::{github::Repository, options::DOWNLOAD};

/// A file to upload as a GitHub Release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upload {
    pub(crate) name: String,
    pub(crate) contents: Vec<u8>,
}

/// Inputs for publishing a single GitHub Release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) version: Version,

    /// Commit hash that tag `v<version>` on GitHub must point to.
    pub(crate) commit: String,

    /// Rendered release description.
    pub(crate) body: String,

    /// Assets to attach, with unique filenames.
    pub(crate) uploads: Vec<Upload>,
}

impl Plan {
    pub(crate) fn tag(&self) -> String {
        format!("v{}", self.version)
    }
}

/// Outcome of publishing a GitHub Release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Published {
    pub(crate) url: String,
    pub(crate) created: bool,
    pub(crate) uploaded: Vec<String>,
    pub(crate) replaced: Vec<String>,
    pub(crate) kept: Vec<String>,
    pub(crate) latest: Version,
}

/// Load and validate the `release`-profile build of `version` stored in `dir`,
/// returning its commit hash and the files to upload (archives,
/// `checksums.txt`, and `manifest.json`).
pub(crate) fn read_build(
    dir: &Utf8Path,
    version: &Version,
) -> Result<(String, Vec<Upload>), String> {
    let again = format!("build it again with `jp release build {version}`");
    if !dir.is_dir() {
        return Err(format!(
            "No build of {version} is kept here; build one with `jp release build {version}`."
        ));
    }

    let manifest = read_manifest(dir).map_err(|problem| format!("{problem} {again}."))?;
    if manifest.version != version.to_string() {
        return Err(format!(
            "The build kept for {version} names version {}; {again}.",
            manifest.version
        ));
    }
    if manifest.profile != Profile::Release.name() {
        return Err(format!(
            "The build of {version} kept here was made with the {} profile, which isn't \
             published; build it with `jp release build {version}`.",
            manifest.profile
        ));
    }

    let problems = check_files(dir, &manifest);
    if !problems.is_empty() {
        return Err(format!(
            "The build of {version} kept here changed since it was made, so {again}:\n{}",
            list(&problems)
        ));
    }

    let mut names: Vec<&str> = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.file.as_str())
        .collect();
    names.extend([CHECKSUMS, MANIFEST]);

    let mut uploads = vec![];
    for name in names {
        let contents = fs::read(dir.join(name)).map_err(|error| {
            format!("`{name}` of the build of {version} can't be read: {error}.")
        })?;
        uploads.push(Upload {
            name: name.to_owned(),
            contents,
        });
    }

    Ok((manifest.commit, uploads))
}

/// Load the workspace-relative `assets` from `root`, substituting `{download}`
/// with `download` in UTF-8 files and leaving binary files unchanged.
pub(crate) fn assets(
    root: &Utf8Path,
    assets: &[impl AsRef<Utf8Path>],
    download: &str,
) -> Result<Vec<Upload>, String> {
    let mut uploads = vec![];
    for asset in assets {
        let asset = asset.as_ref();
        let path = root.join(asset);
        let name = path
            .file_name()
            .ok_or_else(|| {
                format!(
                    "`plugins.command.release-github.options.assets` lists `{asset}`, which names \
                     no file."
                )
            })?
            .to_owned();
        let contents = fs::read(&path).map_err(|error| {
            format!(
                "`{asset}`, which `plugins.command.release-github.options.assets` lists, can't be \
                 read: {error}."
            )
        })?;

        let contents = match String::from_utf8(contents) {
            Ok(text) => text.replace(DOWNLOAD, download).into_bytes(),
            Err(error) => error.into_bytes(),
        };
        uploads.push(Upload { name, contents });
    }

    Ok(uploads)
}

/// Return an error if any two `uploads` share the same filename.
pub(crate) fn distinct(uploads: &[Upload]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let twice: BTreeSet<&str> = uploads
        .iter()
        .filter(|upload| !seen.insert(upload.name.as_str()))
        .map(|upload| upload.name.as_str())
        .collect();
    if twice.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Two files would be attached as {}; rename one of the files \
         `plugins.command.release-github.options.assets` lists.",
        twice
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(" and as ")
    ))
}

/// Publish `plan` to `repository` via `client`, calling `progress` before each
/// asset upload.
///
/// Verifies that tag `v<version>` already exists on GitHub at `plan.commit` so
/// creating the release does not implicitly create a tag on the default branch.
pub(crate) async fn publish(
    client: &Octocrab,
    repository: &Repository,
    plan: Plan,
    progress: &mut dyn FnMut(&str),
) -> Result<Published, String> {
    let tag = plan.tag();
    check_tag(client, repository, &tag, &plan.commit).await?;

    let releases = client
        .releases(&repository.owner, &repository.name)
        .list()
        .await
        .map_err(|error| format!("The Releases of {} can't be read: {error}", repository.web))?;
    let existing = releases.iter().find(|release| release.tag_name == tag);

    let (release, created) = open(client, repository, existing, &plan.version, plan.body).await?;

    let attaching: BTreeSet<&str> = plan
        .uploads
        .iter()
        .map(|upload| upload.name.as_str())
        .collect();
    let kept: Vec<String> = release
        .assets
        .iter()
        .filter(|asset| !attaching.contains(asset.name.as_str()))
        .map(|asset| asset.name.clone())
        .collect();

    let (uploaded, replaced) = attach(client, repository, &release, plan.uploads, progress).await?;
    let (release, latest) =
        release_as_latest(client, repository, &release, &releases, &plan.version).await?;

    Ok(Published {
        url: release.html_url,
        created,
        uploaded,
        replaced,
        kept,
        latest,
    })
}

/// Create a draft release for `version` or update `existing`'s title and body,
/// returning `(release, created)`.
async fn open(
    client: &Octocrab,
    repository: &Repository,
    existing: Option<&Release>,
    version: &Version,
    body: String,
) -> Result<(Release, bool), String> {
    let handler = client.releases(&repository.owner, &repository.name);
    let title = format!("Release {version}");

    match existing {
        None => handler
            .create(&NewRelease {
                tag_name: format!("v{version}"),
                name: title,
                body,
                draft: true,
                make_latest: None,
            })
            .await
            .map(|draft| (draft, true))
            .map_err(|error| format!("The Release of v{version} can't be created: {error}")),
        Some(existing) => handler
            .update(existing.id, &ReleaseUpdate {
                name: Some(title),
                body: Some(body),
                ..ReleaseUpdate::default()
            })
            .await
            .map(|updated| (updated, false))
            .map_err(|error| format!("The Release of v{version} can't be updated: {error}")),
    }
}

/// Upload `uploads` to `release`, deleting any existing assets with the same
/// filename first, and return `(uploaded_names, replaced_names)`.
async fn attach(
    client: &Octocrab,
    repository: &Repository,
    release: &Release,
    uploads: Vec<Upload>,
    progress: &mut dyn FnMut(&str),
) -> Result<(Vec<String>, Vec<String>), String> {
    let handler = client.releases(&repository.owner, &repository.name);

    let mut uploaded = vec![];
    let mut replaced = vec![];
    for upload in uploads {
        if let Some(earlier) = release
            .assets
            .iter()
            .find(|asset| asset.name == upload.name)
        {
            handler
                .delete_asset(earlier.id)
                .await
                .map_err(|error| format!("`{}` can't be replaced: {error}", upload.name))?;
            replaced.push(upload.name.clone());
        }

        progress(&upload.name);
        handler
            .upload_asset(release, &upload.name, upload.contents)
            .await
            .map_err(|error| format!("`{}` can't be attached: {error}", upload.name))?;
        uploaded.push(upload.name);
    }

    Ok((uploaded, replaced))
}

/// Undraft `release` (if it is a draft) and ensure the highest published
/// semantic version between `version` and `releases` is marked as `latest`.
async fn release_as_latest(
    client: &Octocrab,
    repository: &Repository,
    release: &Release,
    releases: &[Release],
    version: &Version,
) -> Result<(Release, Version), String> {
    let handler = client.releases(&repository.owner, &repository.name);
    let tag = format!("v{version}");

    let higher = highest(releases, &tag).filter(|(highest, _)| highest > version);
    let make_latest = if higher.is_some() {
        MakeLatest::False
    } else {
        MakeLatest::True
    };
    let published = handler
        .update(release.id, &ReleaseUpdate {
            draft: release.draft.then_some(false),
            make_latest: Some(make_latest),
            ..ReleaseUpdate::default()
        })
        .await
        .map_err(|error| format!("The Release of {tag} can't be published: {error}"))?;

    let Some((latest, id)) = higher else {
        return Ok((published, version.clone()));
    };
    handler
        .update(id, &ReleaseUpdate {
            make_latest: Some(MakeLatest::True),
            ..ReleaseUpdate::default()
        })
        .await
        .map_err(|error| format!("The Release of v{latest} can't be marked the latest: {error}"))?;

    Ok((published, latest))
}

/// Verify that `tag` exists on GitHub and resolves to `commit`.
async fn check_tag(
    client: &Octocrab,
    repository: &Repository,
    tag: &str,
    commit: &str,
) -> Result<(), String> {
    let sha = client
        .repos(&repository.owner, &repository.name)
        .commit_sha(&format!("tags/{tag}"))
        .await
        .map_err(|error| match error {
            Error::GitHub { source, .. } if matches!(source.status_code.as_u16(), 404 | 422) => {
                format!(
                    "{} has no tag {tag}; push it first, with the `git push` that `jp release \
                     finalize` printed.",
                    repository.web
                )
            }
            error => format!("{tag} can't be read from {}: {error}", repository.web),
        })?;

    if sha == commit {
        return Ok(());
    }

    Err(format!(
        "{tag} on {} names commit {}, but the build kept here is of {}; build it again with `jp \
         release build {}`.",
        repository.web,
        short(&sha),
        short(commit),
        tag.trim_start_matches('v')
    ))
}

/// Return the highest published non-prerelease version in `releases` (excluding
/// `tag`) and its GitHub Release ID.
pub(crate) fn highest(releases: &[Release], tag: &str) -> Option<(Version, u64)> {
    releases
        .iter()
        .filter(|release| release.tag_name != tag && !release.draft && !release.prerelease)
        .filter_map(|release| {
            let version = version_of(&release.tag_name)?;
            version.pre.is_empty().then_some((version, release.id))
        })
        .max_by(|(version, _), (other, _)| version.cmp(other))
}

pub(crate) fn version_of(tag: &str) -> Option<Version> {
    tag.strip_prefix('v')?.parse().ok()
}

pub(crate) fn summary(published: &Published, version: &Version) -> String {
    let verb = if published.created {
        "Published"
    } else {
        "Updated"
    };
    let mut text = format!(
        "{verb} {version} at {}, with {}:\n\n{}\n",
        published.url,
        count(published.uploaded.len(), "file"),
        published
            .uploaded
            .iter()
            .map(|name| format!("    {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if !published.replaced.is_empty() {
        let _ = write!(
            text,
            "\nReplaced {}, which an earlier publish attached.\n",
            names(&published.replaced)
        );
    }
    if !published.kept.is_empty() {
        let _ = write!(
            text,
            "\nLeft {} attached, which this build doesn't have.\n",
            names(&published.kept)
        );
    }

    if published.latest == *version {
        let _ = write!(text, "\n{version} is the latest release.\n");
    } else {
        let _ = write!(
            text,
            "\n{} stays the latest release, as the highest version.\n",
            published.latest
        );
    }

    text
}

fn names(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn list(problems: &[String]) -> String {
    problems
        .iter()
        .map(|problem| format!("- {problem}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
