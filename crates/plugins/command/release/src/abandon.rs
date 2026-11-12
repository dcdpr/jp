//! `jp release abandon`: take back a release nobody has published, so it can be
//! cut again.
//!
//! Releases are abandoned newest first, and one `origin` has tagged is
//! published, which nothing takes back.
//! The release's tag and notes branch are deleted here, with its release
//! branch, or for a patch release the commits from the one that started it.
//! The fragments and highlights its notes filed move back to the top of the
//! fragments directory, with their wording as edited, on
//! `release-notes/abandon-v<version>`: branched from the notes branch while
//! `main` lacks its notes, and from the checkout once it has them.
//! A file `pick` copied from another release is removed instead, and
//! `overview.md` stays in the release's directory for the next cut of the
//! version.
//! When `main` has the notes at the top already, as when a notes branch nobody
//! edited never merged, no abandon branch is made.
//! Nothing is pushed: the command prints the pushes that take the release back
//! on `origin`.

use std::{fs, iter};

use camino::Utf8Path;
use changelog::{
    Place,
    directory::{self, Contents, Part, ReadError},
    fragment, highlight,
};
use semver::Version;

use crate::{
    pick,
    pipeline::{Pipeline, counted, files, listed, problems, short},
    releases::{self, Line, Releases},
    repository::{MAIN, ORIGIN, Repository, Worktree},
};

/// What the abandon branch does to the files of the release's directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Moved {
    /// Fragments moved back to the top level.
    fragments: usize,

    /// Highlights moved back to the top level.
    highlights: usize,

    /// Files `pick` copied from another release, removed.
    copies: usize,
}

/// What happens to one file of the release's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Move {
    /// The file's path below the fragments directory.
    from: String,

    /// Where it goes below the fragments directory, or `None` to remove a file
    /// `pick` copied.
    to: Option<String>,

    highlight: bool,
}

/// What `abandon` took back in this repository, and what is left to take back
/// on `origin`.
#[derive(Debug, Default)]
struct TakenBack {
    /// What was deleted here, each as a sentence names it, such as
    /// `release/0.1` or `the tag v0.1.0`.
    deleted: Vec<String>,

    /// The release branch whose commits came off, and the commit it is back at.
    reset: Option<(String, String)>,

    /// The branches to delete on `origin`.
    remove: Vec<String>,

    /// The release branch to force-push to `origin`, which still has the
    /// commits that came off it here.
    force: Option<String>,
}

/// Take back release `version`, and say what was undone and how to undo it on
/// `origin`.
///
/// The error says why `version` can't be abandoned.
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

    // The notes come back before anything is deleted, so a failure leaves the
    // release as it was.
    let moved = return_notes(pipeline, version, &checkout.head)?;
    let taken = take_back(pipeline, &releases, version)?;

    Ok(report(pipeline, version, &taken, moved))
}

/// Refuse to abandon `version` when `origin` has its tag, when nothing cut it,
/// when a later release exists, while a pick into it is stopped, or when its
/// abandon branch exists already.
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

    if let Some((stopped, worktree)) = pick::stopped(pipeline)?
        && stopped == version.to_string()
    {
        return Err(format!(
            "A pick into {version} stopped at a conflict in {worktree}; finish it with `jp \
             release pick --continue`, or remove that worktree, before abandoning {version}."
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

/// Move the notes of `version` back to the top of the fragments directory on
/// its abandon branch, as `main`, the commit `head`, will merge them; `None`
/// when `head` has them there already.
///
/// The branch starts from the release's notes branch while `head` lacks files
/// of the release's directory that branch has, and from `head` otherwise.
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
         {version}, so it can be {}.\n\nThis {}.\n",
        again(version),
        moved.changes(pipeline)
    );
    let commit = worktree.commit(&message)?;

    // From a notes branch `main` never merged, notes nobody edited come back
    // as `main` has them already.
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

/// Move the fragments and highlights in the directory of `version` in
/// `worktree` back to the top of the fragments directory, removing the ones
/// `pick` copied from another release, and stage it.
///
/// Refuses, before moving anything, a file whose place at the top level holds
/// another file already.
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

/// The fragments and highlights the directory of `version` holds in the
/// fragments directory `dir` of `worktree`; `None` when it has no such
/// directory.
///
/// The error lists the files that break their format.
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

/// Where each file of `contents`, the directory of `version` in the fragments
/// directory `dir`, goes: back to the top level, or nowhere for a copy `pick`
/// made.
///
/// Refuses a file whose place at the top level holds another file already.
fn plan_moves(
    pipeline: &Pipeline<'_>,
    dir: &Utf8Path,
    version: &Version,
    contents: &Contents,
) -> Result<Vec<Move>, String> {
    let others: Vec<Version> = directory::releases(dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|other| other != version)
        .collect();
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
        let text = fs::read_to_string(dir.join(path))
            .map_err(|error| format!("failed to read {}/{path}: {error}", pipeline.fragments))?;
        let top = Place {
            part: Part::Unreleased,
            ..place.clone()
        };

        let to = if is_copy(dir, &place, &others, highlight, &text) {
            None
        } else if dir.join(top.path()).exists() {
            taken.push(format!("{}/{}", pipeline.fragments, top.path()));
            continue;
        } else {
            Some(top.path())
        };
        plan.push(Move {
            from: path.to_owned(),
            to,
            highlight,
        });
    }
    refuse_taken(version, &taken)?;

    Ok(plan)
}

/// Whether the file at `place`, which says `text`, is a copy `pick` made: a
/// file at the same place at the top level or in one of the releases `others`
/// says the same.
fn is_copy(dir: &Utf8Path, place: &Place, others: &[Version], highlight: bool, text: &str) -> bool {
    let top = Place {
        part: Part::Unreleased,
        ..place.clone()
    };
    let mut originals = iter::once(top).chain(others.iter().map(|other| Place {
        part: Part::Release(other.clone()),
        ..place.clone()
    }));

    originals.any(|original| {
        fs::read_to_string(dir.join(original.path()))
            .is_ok_and(|other| alike(&place.name, highlight, text, &other))
    })
}

/// Refuse to move the files of `version` back to the top level when the paths
/// `taken` there hold other files already.
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

/// Make the moves of `plan` in `worktree` and stage them.
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
        let Some(to) = to else {
            worktree.git(&["rm", "--quiet", "--", &from])?;
            moved.copies += 1;
            continue;
        };

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

/// Delete the tag and notes branch of `version` here, and its release branch,
/// or for a patch release the commits from the one that started it, and say
/// what is left to take back on `origin`.
fn take_back(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    version: &Version,
) -> Result<TakenBack, String> {
    let repo = &pipeline.repo;
    let mut taken = TakenBack::default();

    if let Some(line) = releases.line(version) {
        take_back_line(repo, line, version, &mut taken)?;
    }
    delete_branch(repo, &releases::notes_branch(version), &mut taken)?;

    let tag = releases::tag(version);
    if repo.refs.local_tag(&tag)?.is_some() {
        repo.refs.delete_tag(&tag)?;
        taken.deleted.push(format!("the tag {tag}"));
    }

    Ok(taken)
}

/// Take `version` off its release branch `line`: the whole branch for the first
/// release on it, and for a patch release the commits from the one that started
/// it.
fn take_back_line(
    repo: &Repository<'_>,
    line: &Line,
    version: &Version,
    taken: &mut TakenBack,
) -> Result<(), String> {
    if version.patch == 0 {
        // The first release on its branch, and with no later one, the only
        // one.
        return delete_branch(repo, &line.branch, taken);
    }

    // A patch release tagged by hand, with no commit starting it, has nothing
    // on its branch to take back.
    let Some(started) = line
        .started
        .iter()
        .find(|started| started.version == *version)
    else {
        return Ok(());
    };

    let branch = repo.refs.branch(&line.branch)?;
    let before = repo.history.commit(&format!("{}^", started.hash))?;
    repo.refs
        .set_branch(&line.branch, &before, branch.local.as_deref())?;
    if let Some(origin) = &branch.origin
        && repo.history.is_ancestor(&started.hash, origin)?
    {
        taken.force = Some(line.branch.clone());
    }
    taken.reset = Some((line.branch.clone(), before));

    Ok(())
}

/// Delete the branch `name` here when it is here, and list it to delete on
/// `origin` when it is there.
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

/// What abandoning `version` took back, and the pushes and merge left to do.
fn report(
    pipeline: &Pipeline<'_>,
    version: &Version,
    taken: &TakenBack,
    moved: Option<Moved>,
) -> String {
    let abandon = releases::abandon_branch(version);
    let mut said = undone(version, taken);
    said.push_str(&match moved {
        Some(moved) => format!("On {abandon}, {}.\n", moved.describe(pipeline)),
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
    if let Some(branch) = &taken.force {
        commands.push(format!("git push --force-with-lease {ORIGIN} {branch}"));
    }
    if moved.is_some() {
        commands.push(format!("git push {ORIGIN} {abandon}"));
    }

    let again = again(version);
    if commands.is_empty() {
        // Without an abandon branch to merge, nothing is left to do.
        said.push_str(&format!("\n{version} can be {again}.\n"));
        return said;
    }

    said.push_str("\nNothing is pushed yet. Run:\n\n");
    for command in &commands {
        said.push_str(&format!("    {command}\n"));
    }
    said.push_str(&if moved.is_some() {
        format!("\nthen merge {abandon} into {MAIN}, so {version} can be {again}.\n")
    } else {
        format!("\nthen {version} can be {again}.\n")
    });

    said
}

/// The sentence saying what abandoning `version` undid here.
fn undone(version: &Version, taken: &TakenBack) -> String {
    let mut done = vec![];
    if let Some((branch, before)) = &taken.reset {
        done.push(format!(
            "took its commits off {branch}, which is back at {}",
            short(before)
        ));
    }
    if !taken.deleted.is_empty() {
        done.push(format!("deleted {} here", listed(&taken.deleted)));
    }

    if done.is_empty() {
        format!("Abandoned {version}.\n")
    } else {
        format!("Abandoned {version}: {}.\n", done.join(", and "))
    }
}

/// How release `version` starts again once abandoned: a patch release with a
/// pick, any other with a cut.
fn again(version: &Version) -> &'static str {
    if version.patch == 0 {
        "cut again"
    } else {
        "picked into again"
    }
}

/// Whether the texts `copy` and `original` of two files named `name` say the
/// same, whatever credits either lists: what makes `copy` a copy `pick` made.
///
/// Texts that don't follow their format are not alike.
fn alike(name: &str, highlight: bool, copy: &str, original: &str) -> bool {
    if highlight {
        let (Ok(copy), Ok(original)) = (highlight::parse(copy), highlight::parse(original)) else {
            return false;
        };

        return copy.title == original.title
            && copy.authors == original.authors
            && copy.body == original.body;
    }

    let Ok((_, type_)) = fragment::name(name) else {
        return false;
    };
    let (Ok(copy), Ok(original)) = (
        fragment::parse(type_, copy),
        fragment::parse(type_, original),
    ) else {
        return false;
    };

    copy.entry == original.entry && copy.upgrade == original.upgrade
}

impl Moved {
    /// What the abandon branch's commit does, as a clause, such as "moves the 2
    /// fragments its notes filed back to the top of `changelog.d/`".
    fn changes(self, pipeline: &Pipeline<'_>) -> String {
        self.clauses(
            |files| {
                format!(
                    "moves the {files} its notes filed back to the top of `{}/`",
                    pipeline.fragments
                )
            },
            |copies| format!("removes the {copies} `jp release pick` copied from other releases"),
        )
    }

    /// What was done, as a clause, such as "moved 2 fragments back to the top
    /// of changelog.d/".
    fn describe(self, pipeline: &Pipeline<'_>) -> String {
        self.clauses(
            |files| format!("moved {files} back to the top of {}/", pipeline.fragments),
            |copies| format!("removed {copies} `jp release pick` copied from other releases"),
        )
    }

    /// The clauses for what moved and what was removed, joined with "and".
    ///
    /// `moved` words the fragments and highlights moved back, given them
    /// counted, such as "2 fragments"; `removed` words the copies removed.
    /// A clause with nothing to count is left out.
    fn clauses(
        self,
        moved: impl FnOnce(String) -> String,
        removed: impl FnOnce(String) -> String,
    ) -> String {
        let mut parts = vec![];
        if self.fragments + self.highlights > 0 {
            parts.push(moved(files(self.fragments, self.highlights)));
        }
        if self.copies > 0 {
            parts.push(removed(counted(self.copies, "file", "files")));
        }

        parts.join(", and ")
    }
}

#[cfg(test)]
#[path = "abandon_tests.rs"]
mod tests;
