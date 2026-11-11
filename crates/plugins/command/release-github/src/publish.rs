//! Publishing a release to GitHub: its build's files and the workspace's
//! assets, attached to the Release of its tag, with its notes as the
//! description.
//!
//! A new Release is created as a draft, given its files, and then published, so
//! it never shows without them.
//! An existing Release is updated in place: its title and description are
//! replaced, and so is each file of a name this publish attaches; files of
//! other names stay.
//! The Release of the highest version is marked the latest each time, since
//! GitHub would otherwise make the most recently published one the latest.

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

/// A file to attach to a Release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upload {
    pub(crate) name: String,
    pub(crate) contents: Vec<u8>,
}

/// A release to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) version: Version,

    /// The commit the release's build is of, which its tag on GitHub has to
    /// name.
    pub(crate) commit: String,

    /// The Release's description.
    pub(crate) body: String,

    /// The files to attach, each name once.
    pub(crate) uploads: Vec<Upload>,
}

impl Plan {
    /// The tag of the release, such as `v0.3.0`.
    pub(crate) fn tag(&self) -> String {
        format!("v{}", self.version)
    }
}

/// What a publish did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Published {
    /// The Release's page.
    pub(crate) url: String,

    /// Whether the publish created the Release.
    pub(crate) created: bool,

    /// The files attached, in the order they were.
    pub(crate) uploaded: Vec<String>,

    /// The files an earlier publish attached that were replaced.
    pub(crate) replaced: Vec<String>,

    /// The files the Release has that this publish didn't attach.
    pub(crate) kept: Vec<String>,

    /// The version of the Release marked the latest.
    pub(crate) latest: Version,
}

/// The files of the publishable build of `version` kept in `dir`, and the
/// commit it was built from.
///
/// The build has to be of `version`, made with the `release` profile, and
/// unchanged since: every file's SHA-256 has to match its `checksums.txt`.
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

/// The files `assets` names, relative to the workspace at `root`, each with
/// every `{download}` in it replaced by `download`.
///
/// A file that isn't UTF-8 text is attached as it is.
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

/// Refuse two files that would be attached under one name.
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

/// Publish `plan` as the Release of its tag in `repository`, through `client`.
///
/// `progress` is told of each file as it is attached.
/// The tag has to be on GitHub already, at the commit the build is of: a
/// Release of a tag GitHub doesn't have would make one, at the tip of the
/// default branch.
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

/// The Release of `version`: `existing`, retitled and given `body`, or else a
/// new draft; and whether it is new.
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

/// Attach `uploads` to `release`, each in place of a file of its name the
/// Release has; return the names attached and the names replaced.
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

/// Publish `release`, the Release of `version`, if it is a draft, and mark the
/// highest version among it and the published `releases` the latest; return the
/// Release as published and the version marked.
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

/// Refuse to publish unless GitHub has `tag`, and it names `commit`.
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

/// The highest version of the published Releases other than the one of `tag`,
/// and that Release's id.
///
/// Drafts, pre-releases, and Releases of tags that aren't `v<version>` don't
/// count.
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

/// The version a tag `v<version>` names.
pub(crate) fn version_of(tag: &str) -> Option<Version> {
    tag.strip_prefix('v')?.parse().ok()
}

/// What to say of a publish of `version`.
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

    // Writing to a `String` can't fail.
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

/// `names`, each in backticks, joined into a list a sentence can hold.
fn names(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `count` of `noun`, as `1 file` or `9 files`.
fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Each problem on a line of its own, as a list.
fn list(problems: &[String]) -> String {
    problems
        .iter()
        .map(|problem| format!("- {problem}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first 7 characters of a commit's hash.
fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
