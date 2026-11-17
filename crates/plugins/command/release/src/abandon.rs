//! `jp release abandon`: retract an unpublished release so it can be cut again.
//!
//! Releases must be abandoned from newest to oldest, and any release whose tag
//! has been pushed to `origin` cannot be abandoned.
//!
//! Abandoning a release:
//!
//! - Deletes the local tag `v<version>`, notes branch
//!   `release-notes/v<version>`, and release branch `release/<major>.<minor>`.
//! - Creates `release-notes/abandon-v<version>` moving the release's fragments
//!   and highlights back to the top level of the fragments directory (leaving
//!   `overview.md` and `.order.json` in place).
//!   If `main` already has the files at the top level unedited, no abandon
//!   branch is created.
//! - Prints the `git push` commands needed to remove the remote branches.

use std::fs;

use camino::Utf8Path;
use changelog::{
    Place,
    directory::{self, Contents, Part, ReadError},
};
use semver::Version;

use crate::{
    pipeline::{Pipeline, files, listed, problems},
    releases::{self, Releases},
    repository::{MAIN, ORIGIN, Repository, Worktree},
};

/// Counts of files moved on `release-notes/abandon-v<version>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Moved {
    fragments: usize,
    highlights: usize,
}

struct Move {
    from: String,

    /// Destination path relative to the fragments directory.
    to: String,

    highlight: bool,
}

/// Local ref changes performed by `abandon` and the remaining remote ref
/// updates to print.
#[derive(Debug, Default)]
struct TakenBack {
    deleted: Vec<String>,
    remove: Vec<String>,
}

/// Abandon release `version` locally and return a summary with the `git push`
/// commands to update `origin`.
pub(crate) fn abandon(pipeline: &Pipeline<'_>, version: &Version) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.origin.fetch()?;
    let releases = repo.releases()?;
    refuse(pipeline, &releases, version)?;
    let checkout = repo.origin.checkout()?;
    repo.refs
        .refuse_checked_out(&releases::notes_branch(version))?;
    if let Some(line) = releases.line(version) {
        repo.refs.refuse_checked_out(&line.branch)?;
    }

    // Prepare the abandon branch before deleting refs so a failure leaves the
    // release untouched.
    let moved = return_notes(pipeline, version, &checkout.head)?;
    let taken = take_back(pipeline, &releases, version)?;

    Ok(report(pipeline, version, &taken, moved))
}

fn refuse(pipeline: &Pipeline<'_>, releases: &Releases, version: &Version) -> Result<(), String> {
    let tag = releases::tag(version);
    if releases.published.contains(version) {
        return Err(format!(
            "{ORIGIN} has the tag {tag}, so {version} is published, and a published release can't \
             be taken back."
        ));
    }

    let started = releases.line(version).is_some_and(|line| {
        line.started
            .iter()
            .any(|started| started.version == *version)
    });
    if !started && !releases.tagged.contains(version) {
        return Err(format!(
            "{version} hasn't been cut, so there is nothing to abandon."
        ));
    }

    if let Some(latest) = releases.latest().filter(|latest| *latest != version) {
        return Err(format!(
            "{latest} is newer, and releases are abandoned newest first: abandon {latest}, and \
             run the pushes that prints, before {version}."
        ));
    }

    let name = releases::abandon_branch(version);
    let existing = pipeline.repo.refs.branch(&name)?;
    if existing.local.is_some() || existing.origin.is_some() {
        return Err(format!(
            "{name} exists already; delete it if an earlier abandon left it behind, here with \
             `git branch -D {name}` or on {ORIGIN} with `git push {ORIGIN} --delete {name}`."
        ));
    }

    Ok(())
}

/// Create `release-notes/abandon-v<version>` moving `version`'s fragments and
/// highlights back to the top level of the fragments directory, or return
/// `Ok(None)` if `head` already has those files at the top level.
fn return_notes(
    pipeline: &Pipeline<'_>,
    version: &Version,
    head: &str,
) -> Result<Option<Moved>, String> {
    let repo = &pipeline.repo;
    let notes = repo.refs.branch(&releases::notes_branch(version))?;
    let directory = format!("{}/{version}/", pipeline.fragments);
    let filed = repo.history.files_at(head, &directory)?;
    let base = match repo.refs.tip(&notes)? {
        Some(tip) if !repo.history.files_at(&tip, &directory)?.is_subset(&filed) => tip,
        _ => head.to_owned(),
    };

    let worktree = repo.worktrees.add("abandon", &base)?;
    let moved = move_back(pipeline, &worktree, version)?;
    if moved == Moved::default() {
        return Ok(None);
    }
    let message = format!(
        "chore(release): Abandon release {version}\n\n`jp release abandon` took back release \
         {version}, so it can be cut again.\n\nThis moves the {} its notes filed back to the top \
         of `{}/`.\n",
        files(moved.fragments, moved.highlights),
        pipeline.fragments
    );
    let commit = worktree.commit(&message)?;

    let before = repo.history.merge_base(head, &base)?;
    if repo
        .history
        .same_below(&before, &commit, &pipeline.fragments)?
    {
        return Ok(None);
    }
    repo.refs
        .set_branch(&releases::abandon_branch(version), &commit, None)?;

    Ok(Some(moved))
}

fn move_back(
    pipeline: &Pipeline<'_>,
    worktree: &Worktree<'_>,
    version: &Version,
) -> Result<Moved, String> {
    let dir = pipeline.fragments_in(worktree);
    if !dir.is_dir() {
        return Ok(Moved::default());
    }
    let Some(contents) = release_contents(worktree, &dir, version)? else {
        return Ok(Moved::default());
    };
    let plan = plan_moves(pipeline, &dir, version, &contents)?;

    apply_moves(pipeline, worktree, plan)
}

fn release_contents(
    worktree: &Worktree<'_>,
    dir: &Utf8Path,
    version: &Version,
) -> Result<Option<Contents>, String> {
    match directory::read(dir, &Part::Release(version.clone())) {
        Ok(contents) => Ok(Some(contents)),
        Err(ReadError::NoRelease { .. }) => Ok(None),
        Err(ReadError::Problems(found)) => Err(format!(
            "The notes of {version} break their format; fix these first:\n{}",
            problems(worktree, &found)
        )),
        Err(error) => Err(error.to_string()),
    }
}

fn plan_moves(
    pipeline: &Pipeline<'_>,
    dir: &Utf8Path,
    version: &Version,
    contents: &Contents,
) -> Result<Vec<Move>, String> {
    let paths = contents
        .fragments
        .iter()
        .map(|fragment| (fragment.path.as_str(), false))
        .chain(
            contents
                .highlights
                .iter()
                .map(|highlight| (highlight.path.as_str(), true)),
        );

    let mut plan = vec![];
    let mut taken = vec![];
    for (path, highlight) in paths {
        let Some(place) = Place::parse(path) else {
            continue;
        };
        let top = Place {
            part: Part::Unreleased,
            ..place
        };

        if dir.join(top.path()).exists() {
            taken.push(format!("{}/{}", pipeline.fragments, top.path()));
            continue;
        }
        plan.push(Move {
            from: path.to_owned(),
            to: top.path(),
            highlight,
        });
    }
    refuse_taken(version, &taken)?;

    Ok(plan)
}

fn refuse_taken(version: &Version, taken: &[String]) -> Result<(), String> {
    if taken.is_empty() {
        return Ok(());
    }

    let (exist, which, names, rename) = if taken.len() == 1 {
        ("exists", "the file", "its name", "one of the two")
    } else {
        ("exist", "the files", "their names", "one of each pair")
    };

    Err(format!(
        "{} {exist} already, so {which} of {version} with {names} can't move back there; rename \
         {rename}, and abandon again.",
        listed(taken)
    ))
}

fn apply_moves(
    pipeline: &Pipeline<'_>,
    worktree: &Worktree<'_>,
    plan: Vec<Move>,
) -> Result<Moved, String> {
    let mut moved = Moved::default();
    for Move {
        from,
        to,
        highlight,
    } in plan
    {
        let from = format!("{}/{from}", pipeline.fragments);
        let to = format!("{}/{to}", pipeline.fragments);
        if let Some(parent) = worktree.workspace().join(&to).parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {parent}: {error}"))?;
        }
        worktree.git(&["mv", &from, &to])?;
        if highlight {
            moved.highlights += 1;
        } else {
            moved.fragments += 1;
        }
    }

    Ok(moved)
}

fn take_back(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    version: &Version,
) -> Result<TakenBack, String> {
    let repo = &pipeline.repo;
    let mut taken = TakenBack::default();

    if let Some(line) = releases.line(version) {
        delete_branch(repo, &line.branch, &mut taken)?;
    }
    delete_branch(repo, &releases::notes_branch(version), &mut taken)?;

    let tag = releases::tag(version);
    if repo.refs.local_tag(&tag)?.is_some() {
        repo.refs.delete_tag(&tag)?;
        taken.deleted.push(format!("the tag {tag}"));
    }

    Ok(taken)
}

fn delete_branch(repo: &Repository<'_>, name: &str, taken: &mut TakenBack) -> Result<(), String> {
    let branch = repo.refs.branch(name)?;
    if let Some(local) = &branch.local {
        repo.refs.delete_branch(name, local)?;
        taken.deleted.push(name.to_owned());
    }
    if branch.origin.is_some() {
        taken.remove.push(name.to_owned());
    }

    Ok(())
}

fn report(
    pipeline: &Pipeline<'_>,
    version: &Version,
    taken: &TakenBack,
    moved: Option<Moved>,
) -> String {
    let abandon = releases::abandon_branch(version);
    let mut said = undone(version, taken);
    said.push_str(&match moved {
        Some(moved) => format!(
            "On {abandon}, moved {} back to the top of {}/.\n",
            files(moved.fragments, moved.highlights),
            pipeline.fragments
        ),
        None => format!(
            "None of the notes of {version} has to come back to the top of {}/.\n",
            pipeline.fragments
        ),
    });

    let mut commands = vec![];
    if !taken.remove.is_empty() {
        commands.push(format!(
            "git push {ORIGIN} --delete {}",
            taken.remove.join(" ")
        ));
    }
    if moved.is_some() {
        commands.push(format!("git push {ORIGIN} {abandon}"));
    }

    if commands.is_empty() {
        said.push_str(&format!("\n{version} can be cut again.\n"));
        return said;
    }

    said.push_str("\nNothing is pushed yet. Run:\n\n");
    for command in &commands {
        said.push_str(&format!("    {command}\n"));
    }
    said.push_str(&if moved.is_some() {
        format!("\nthen merge {abandon} into {MAIN}, so {version} can be cut again.\n")
    } else {
        format!("\nthen {version} can be cut again.\n")
    });

    said
}

fn undone(version: &Version, taken: &TakenBack) -> String {
    if taken.deleted.is_empty() {
        format!("Abandoned {version}.\n")
    } else {
        format!(
            "Abandoned {version}: deleted {} here.\n",
            listed(&taken.deleted)
        )
    }
}

#[cfg(test)]
#[path = "abandon_tests.rs"]
mod tests;
