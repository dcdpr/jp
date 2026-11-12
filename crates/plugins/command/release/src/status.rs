//! `jp release status`: where the releases stand, and what to run next.
//!
//! It reads what `cut`, `pick`, and `finalize` read: the release branches,
//! notes branches, and `v` tags here and on `origin`, and the commit checked
//! out, which stands for `main`.
//! It says, in this order: that the checkout lacks commits of `origin`'s
//! `main`; a pick a conflict stopped; what only this repository has, with the
//! push that publishes it; each release in preparation, with where its notes
//! are and the next step; the finalized releases with no publishable build kept
//! in user-local storage; and, while no minor or major release is in
//! preparation, whether the next one is due.
//! It changes nothing.

use camino::Utf8Path;
use semver::Version;

use crate::{
    build::{self, Kept},
    due,
    finalize::{self, Notes},
    pick,
    pipeline::{Pipeline, counted, listed, push},
    releases::{self, Line, NOTES_BRANCH, RELEASE_BRANCH, Releases, Standing},
    repository::{Branch, Checkout, MAIN, ORIGIN, Tags, Worktree},
};

/// Where the releases of the workspace stand at `now`, in seconds since the
/// Unix epoch, and what to run next; `builds` is where builds are kept, when
/// the workspace has user-local storage.
///
/// The error says why the repository can't be read.
pub(crate) fn status(
    pipeline: &Pipeline<'_>,
    now: i64,
    builds: Option<&Utf8Path>,
) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let (releases, tags) = repo.releases_and_tags()?;
    let checkout = repo.origin.checked_out()?;

    let mut paragraphs = vec![];
    if repo.origin.lacks_main(&checkout)? {
        paragraphs.push(checkout.behind());
    }
    if let Some((version, worktree)) = pick::stopped(pipeline)? {
        paragraphs.push(format!(
            "A pick into {version} stopped at a conflict in {worktree}; resolve it there, then \
             run:\n\n    jp release pick --continue"
        ));
    }
    paragraphs.extend(unpublished(pipeline, &checkout, &tags)?);

    let in_preparation: Vec<(&Line, &Version)> = releases
        .lines
        .values()
        .filter_map(|line| Some((line, releases.in_preparation(line)?)))
        .collect();
    // A worktree is a checkout of the whole repository, so one is made only to
    // read the notes of a release in preparation, and is removed before `due`
    // checks out its own.
    if !in_preparation.is_empty() {
        let main = repo.worktrees.add("status", &checkout.head)?;
        for (line, version) in in_preparation {
            paragraphs.push(preparing(
                pipeline, &releases, line, version, &checkout, &main,
            )?);
        }
    }
    if let Some(builds) = builds {
        paragraphs.extend(unbuilt(&releases, &tags, builds));
    }

    let cutting = releases
        .newest_cut()
        .is_some_and(|newest| !releases.tagged.contains(newest));
    if !cutting {
        let next = due::next(pipeline, &releases, now, &checkout.head)?;
        paragraphs.push(if next.due {
            format!("{}\n\nCut it with:\n\n    jp release cut", next.reason)
        } else {
            next.reason
        });
    }

    Ok(paragraphs.join("\n\n") + "\n")
}

/// What only this repository has of the releases, with the push that publishes
/// it: release branches and notes branches with commits `origin` lacks, leaving
/// out notes branches the checkout has merged, and `v` tags.
/// Then the notes branches here and on `origin` that diverged; diverged release
/// branches stop reading the releases before this, saying how to bring them
/// together.
fn unpublished(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    tags: &Tags,
) -> Result<Vec<String>, String> {
    let repo = &pipeline.repo;
    let mut branches = vec![];
    for branch in repo.refs.branches(RELEASE_BRANCH)? {
        if releases::line_of(&branch.name).is_some()
            && repo.refs.standing(&branch)?.is_some_and(Standing::unpushed)
        {
            branches.push(branch.name);
        }
    }
    let (notes, diverged) = notes_branches(pipeline, checkout)?;
    branches.extend(notes);
    let tags: Vec<String> = tags
        .local
        .keys()
        .filter(|name| !tags.origin.contains_key(*name))
        .filter(|name| !releases::versions([*name]).is_empty())
        .cloned()
        .collect();

    let mut paragraphs: Vec<String> = publish_them(branches, tags).into_iter().collect();
    if !diverged.is_empty() {
        let them = if diverged.len() == 1 { "it" } else { "them" };
        paragraphs.push(format!(
            "{} here and on {ORIGIN} each have commits the other lacks; bring {them} together \
             before the next release command.",
            listed(&diverged)
        ));
    }

    Ok(paragraphs)
}

/// The notes branches with commits `origin` lacks, leaving out the ones the
/// commit checked out has merged, and the notes branches here and on `origin`
/// that diverged.
fn notes_branches(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
) -> Result<(Vec<String>, Vec<String>), String> {
    let repo = &pipeline.repo;
    let mut unpushed = vec![];
    let mut diverged = vec![];

    for branch in repo.refs.branches(NOTES_BRANCH)? {
        let Some(version) = notes_version(&branch.name) else {
            continue;
        };
        match repo.refs.standing(&branch)? {
            Some(Standing::Local | Standing::Ahead)
                if !merged(pipeline, checkout, &branch, &version)? =>
            {
                unpushed.push(branch.name);
            }
            Some(Standing::Diverged) => diverged.push(branch.name),
            _ => {}
        }
    }

    Ok((unpushed, diverged))
}

/// Whether the commit checked out has the files the notes branch `branch` of
/// `version` filed, which makes it merged: pushing it again would only bring it
/// back.
///
/// A branch only `origin` has counts as merged.
fn merged(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    branch: &Branch,
    version: &Version,
) -> Result<bool, String> {
    let Some(local) = &branch.local else {
        return Ok(true);
    };
    let repo = &pipeline.repo;
    let directory = format!("{}/{version}/", pipeline.fragments);

    Ok(repo
        .history
        .files_at(local, &directory)?
        .is_subset(&repo.history.files_at(&checkout.head, &directory)?))
}

/// The paragraph saying only this repository has the branches `branches` and
/// the tags `tags`, how to publish them, and how to delete what a rehearsal
/// left; `None` when there are none.
fn publish_them(branches: Vec<String>, tags: Vec<String>) -> Option<String> {
    if branches.is_empty() && tags.is_empty() {
        return None;
    }

    let mut delete = vec![];
    if !branches.is_empty() {
        delete.push(format!("`git branch -D {}`", branches.join(" ")));
    }
    if !tags.is_empty() {
        delete.push(format!("`git tag -d {}`", tags.join(" ")));
    }
    let names: Vec<String> = branches.into_iter().chain(tags).collect();
    let them = if names.len() == 1 { "it" } else { "them" };

    Some(format!(
        "Only this repository has {}. Publish {them} with:\n\n{}\nor, if a rehearsal left {them}, \
         delete {them} with {}.",
        listed(&names),
        push(&names),
        delete.join(" and "),
    ))
}

/// The finalized releases with no publishable build kept in `builds`, and the
/// command that builds one; nothing when every one has.
fn unbuilt(releases: &Releases, tags: &Tags, builds: &Utf8Path) -> Option<String> {
    let mut unbuilt = vec![];
    let mut newest = None;
    for version in &releases.tagged {
        let tag = releases::tag(version);
        // `origin`'s tag is the one published.
        let Some(commit) = tags.origin.get(&tag).or_else(|| tags.local.get(&tag)) else {
            continue;
        };

        match build::assess(builds, version, commit) {
            Kept::Publishable => continue,
            Kept::Nothing => unbuilt.push(version.to_string()),
            Kept::ForTesting(what) => unbuilt.push(format!("{version} (only {what})")),
        }
        newest = Some(version);
    }
    let newest = newest?;

    let how = if unbuilt.len() == 1 {
        "Build one with:"
    } else {
        "Build each with `jp release build <version>`, such as the newest:"
    };

    Some(format!(
        "No publishable build of {} is kept here. {how}\n\n    jp release build {newest}",
        listed(&unbuilt)
    ))
}

/// The version of the release the notes branch `name` files the notes of.
fn notes_version(name: &str) -> Option<Version> {
    let version = name.strip_prefix(NOTES_BRANCH)?.strip_prefix('v')?;

    Version::parse(version).ok()
}

/// Where `version`, in preparation on `line`, stands, and the next step: the
/// notes are read from `checkout`, which `main` is a worktree of.
fn preparing(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    line: &Line,
    version: &Version,
    checkout: &Checkout,
    main: &Worktree<'_>,
) -> Result<String, String> {
    let notes_branch = releases::notes_branch(version);
    let at = checkout.describe();
    let next = match finalize::notes(pipeline, releases, line, version, main)? {
        Notes::Unmerged { branch, .. } => format!(
            "Its notes are on {branch}, and {at} lacks some of them: merge {notes_branch} into \
             {MAIN} once they are final, then finalize it."
        ),
        Notes::Unfiled(pending) => format!(
            "{at} still has {} at the top of {} whose changes {} ships, so its notes aren't filed \
             yet: merge {notes_branch} into {MAIN} from wherever it was made.",
            counted(pending.len(), "file", "files"),
            pipeline.fragments,
            line.branch,
        ),
        Notes::Missing => format!(
            "{at} has no notes of it: write at least one fragment into {}/{version}/ on {MAIN}, \
             then finalize it.",
            pipeline.fragments
        ),
        Notes::Filed => format!(
            "Its notes are on {at}. Once {}'s checks pass, finalize it with:\n\n    jp release \
             finalize {version}",
            line.branch
        ),
    };

    Ok(format!(
        "{version} is in preparation on {}. {next}",
        line.branch
    ))
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
