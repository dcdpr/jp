//! `jp release pick`: add commits from `main` to a release in preparation, or
//! start the next patch release with them.
//!
//! `main` is the commit the workspace's checkout is at, which has to hold every
//! commit of `origin`'s `main`, and the commits are picked from its
//! first-parent line.
//! They are cherry-picked onto the release branch with `git cherry-pick -x`, in
//! the order `main` has them, leaving out what they change in the fragments
//! directory, which a release branch doesn't have.
//! The fragments and highlights they added are filed into the release's
//! directory on its notes branch, `release-notes/v<version>`: the one the cut
//! or an earlier pick made, with `main` merged into it so that it holds them,
//! or a new one from `main`.
//! One that no other release ships moves, so the next release doesn't list the
//! change again; one another release ships too is copied, with its credits
//! written into the copy's `Credits` fields, so both releases list it.
//! Both branches are updated in this repository, and the pick prints the push
//! that publishes them.
//!
//! A conflict stops the pick in its worktree, which is kept for the user to
//! resolve, and `jp release pick --continue` finishes it.
//! What the stopped pick has left to do is kept in the repository's git
//! directory, beside its worktrees.

use std::{collections::HashMap, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{History, Place, directory::Part, fragment, highlight, with_credits};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    pipeline::{Pipeline, counted, files, listed, push, short},
    releases::{self, Line, Releases},
    repository::{Branch, Checkout, MAIN, ORIGIN, Worktree},
};

/// The file, in the repository's git directory, that keeps a stopped pick.
const STOPPED: &str = "jp-release-pick.json";

/// A pick a conflict stopped, as `--continue` finds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stopped {
    /// The worktree holding the release branch with the picks made so far.
    worktree: Utf8PathBuf,

    /// The release the commits go into.
    version: String,

    /// The hashes of the commits to pick, in the order `main` has them.
    commits: Vec<String>,

    /// The index into `commits` of the pick that stopped, or `commits.len()`
    /// once every pick is made.
    stopped: usize,
}

/// A commit to pick.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Commit {
    hash: String,
    subject: String,
}

/// A fragment or highlight a pick files into the release's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Filed {
    /// Where it is on `main`, below the fragments directory.
    from: String,

    /// Where it goes, below the fragments directory.
    to: String,

    highlight: bool,

    /// The copy's text, for a file another release ships too; `None` moves the
    /// file.
    copy: Option<String>,
}

/// What a pick files on the release's notes branch, planned in a worktree at
/// `main` with the notes branch merged in.
struct Filing<'r> {
    notes: Worktree<'r>,

    /// The notes branch, as it was before the pick.
    branch: Branch,

    files: Vec<Filed>,

    /// The files the commits added that `main` no longer has, as notes for the
    /// user.
    gone: Vec<String>,
}

/// Everything a pick needs settled before it changes a branch.
struct Prepared<'r> {
    /// The release branch, at the commit the pick builds on.
    line: Line,

    /// This repository's release branch, as it was before the pick; `None` when
    /// only `origin` has it.
    local: Option<String>,

    /// Whether the pick starts the release.
    start: bool,

    filing: Filing<'r>,
}

/// What became of one cherry-pick.
enum Picked {
    Done,

    /// It stopped at a conflict in these paths, below the top of the work tree.
    Conflicts(Vec<String>),
}

/// Pick `commits` from `main` into release `version`, and say what was done and
/// how to publish it.
///
/// The error says why nothing was picked, or where a conflict stopped the pick.
pub(crate) fn pick(
    pipeline: &Pipeline<'_>,
    version: &Version,
    commits: &[String],
) -> Result<String, String> {
    let repo = &pipeline.repo;
    let state = repo.worktrees.common_dir()?.join(STOPPED);
    if let Some(stopped) = load(&state)? {
        if stopped.worktree.join(".git").exists() {
            return Err(format!(
                "A pick into {} stopped at a conflict in {}; resolve it there and run `jp release \
                 pick --continue`, or drop it with `git worktree remove --force {}`.",
                stopped.version, stopped.worktree, stopped.worktree
            ));
        }
        // Its worktree is gone, so the pick was dropped.
        remove(&state)?;
    }

    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let checkout = repo.origin.checkout()?;
    let releases = repo.releases()?;
    let target = releases.pick_target(version)?;
    let commits = order(pipeline, &checkout, target.line, commits)?;
    let prepared = prepare(pipeline, &checkout, &releases, version, &commits)?;

    let code = repo.worktrees.add("pick", &prepared.line.tip)?;
    if prepared.start {
        if let Some(bump) = &pipeline.options.bump {
            code.bump(bump, version)?;
        }
        code.commit(&releases::start_message(version))?;
    }
    for (index, commit) in commits.iter().enumerate() {
        if let Picked::Conflicts(paths) = pick_one(pipeline, &code, &commit.hash)? {
            return Err(stop(&state, code, version, &commits, index, &paths)?);
        }
    }

    finish(
        pipeline,
        &checkout,
        &prepared,
        &code.head()?,
        version,
        &commits,
    )
}

/// Finish the pick a conflict stopped, once the conflict is resolved, and say
/// what was done and how to publish it.
///
/// The error says why it can't be finished yet; the stopped pick is kept, to
/// try again.
pub(crate) fn resume(pipeline: &Pipeline<'_>) -> Result<String, String> {
    let repo = &pipeline.repo;
    let state = repo.worktrees.common_dir()?.join(STOPPED);
    let Some(stopped) = load(&state)? else {
        return Err("No pick is stopped, so there is nothing to continue.".to_owned());
    };
    let Ok(code) = repo.worktrees.reopen(&stopped.worktree) else {
        remove(&state)?;
        return Err(format!(
            "The worktree the pick into {} stopped in, {}, is gone; pick the commits again.",
            stopped.version, stopped.worktree
        ));
    };
    let version = Version::parse(&stopped.version)
        .map_err(|error| format!("{state} names no version: {error}"))?;
    let commits = stopped
        .commits
        .iter()
        .map(|hash| {
            Ok(Commit {
                hash: hash.clone(),
                subject: repo.history.subject(hash)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let code = pick_rest(pipeline, &state, code, &version, &commits, &stopped)?;
    finish_resumed(pipeline, &state, code, &version, &commits, &stopped)
}

/// Make the picks of `commits` into `version` the stopped pick `stopped` has
/// left to do in `code`, once the one that stopped is resolved.
///
/// Returns `code` with every pick made; the error says what stopped it, with
/// the pick kept in `code` and recorded in `state`.
fn pick_rest<'r>(
    pipeline: &Pipeline<'_>,
    state: &Utf8Path,
    code: Worktree<'r>,
    version: &Version,
    commits: &[Commit],
    stopped: &Stopped,
) -> Result<Worktree<'r>, String> {
    if let Some(commit) = commits.get(stopped.stopped)
        && let Err(error) = resolve(pipeline, &code, &commit.hash)
    {
        code.keep();
        return Err(error);
    }

    for (index, commit) in commits.iter().enumerate().skip(stopped.stopped + 1) {
        match pick_one(pipeline, &code, &commit.hash) {
            Ok(Picked::Done) => {}
            Ok(Picked::Conflicts(paths)) => {
                return Err(stop(state, code, version, commits, index, &paths)?);
            }
            Err(error) => {
                let top = code.keep();
                save(state, &Stopped {
                    stopped: index,
                    ..stopped.clone()
                })?;
                return Err(format!(
                    "{error}\nThe pick is kept in {top}; finish it there and run `jp release pick \
                     --continue`, or drop it with `git worktree remove --force {top}`."
                ));
            }
        }
    }

    Ok(code)
}

/// Update the branches for the stopped pick `stopped`, every pick of which is
/// made in `code`, and say what was done and how to publish it.
///
/// The worktree is kept until the branches are updated; when they can't be, the
/// error says how to try again.
fn finish_resumed(
    pipeline: &Pipeline<'_>,
    state: &Utf8Path,
    code: Worktree<'_>,
    version: &Version,
    commits: &[Commit],
    stopped: &Stopped,
) -> Result<String, String> {
    let repo = &pipeline.repo;
    let finished = (|| {
        repo.origin.refuse_shallow()?;
        repo.origin.fetch()?;
        let checkout = repo.origin.checkout()?;
        let releases = repo.releases()?;
        let prepared = prepare(pipeline, &checkout, &releases, version, commits)?;

        finish(
            pipeline,
            &checkout,
            &prepared,
            &code.head()?,
            version,
            commits,
        )
    })();
    match finished {
        Ok(said) => {
            remove(state)?;
            drop(code);
            Ok(said)
        }
        Err(error) => {
            let top = code.keep();
            save(state, &Stopped {
                stopped: commits.len(),
                ..stopped.clone()
            })?;
            Err(format!(
                "{error}\nThe picks are kept in {top}; run `jp release pick --continue` to try \
                 again, or drop them with `git worktree remove --force {top}`."
            ))
        }
    }
}

/// The commits `commits` name, in the order `main` has them, each once.
///
/// The error names a commit that isn't on the first-parent line of `checkout`,
/// that `line` has already, or that changes nothing but notes.
fn order(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    line: &Line,
    commits: &[String],
) -> Result<Vec<Commit>, String> {
    let repo = &pipeline.repo;
    let main_line = repo.history.first_parents(&checkout.head)?;
    let positions: HashMap<&str, usize> = main_line
        .iter()
        .enumerate()
        .map(|(position, hash)| (hash.as_str(), position))
        .collect();

    let mut ordered: Vec<(usize, Commit)> = vec![];
    for commit in commits {
        let hash = repo.history.commit(commit)?;
        if ordered.iter().any(|(_, known)| known.hash == hash) {
            continue;
        }
        let Some(&position) = positions.get(hash.as_str()) else {
            return Err(format!(
                "`{commit}` isn't on the first-parent line of {}; pick the commit that brought it \
                 to `{MAIN}`, such as the merge of its branch.",
                checkout.describe()
            ));
        };
        if line.picked.contains(&hash) || repo.history.containing(&hash)?.contains(&line.branch) {
            return Err(format!("{} has {} already.", line.branch, short(&hash)));
        }
        refuse_notes_only(pipeline, &hash)?;

        let subject = repo.history.subject(&hash)?;
        ordered.push((position, Commit { hash, subject }));
    }

    // `main_line` lists the newest commit first.
    ordered.sort_by(|(a, _), (b, _)| b.cmp(a));

    Ok(ordered.into_iter().map(|(_, commit)| commit).collect())
}

/// Refuse the commit `hash` when it changes nothing outside the fragments
/// directory.
fn refuse_notes_only(pipeline: &Pipeline<'_>, hash: &str) -> Result<(), String> {
    if pipeline
        .repo
        .history
        .changes_outside(hash, &pipeline.fragments)?
    {
        return Ok(());
    }

    Err(format!(
        "{} changes nothing outside {}, and release notes are corrected on `{MAIN}`, not picked.",
        short(hash),
        pipeline.fragments
    ))
}

/// Settle where the pick of `commits` into `version` goes, and what it files on
/// the release's notes branch.
///
/// Refuses a release branch or notes branch checked out in a worktree, before
/// anything is picked.
fn prepare<'r>(
    pipeline: &'r Pipeline<'_>,
    checkout: &Checkout,
    releases: &Releases,
    version: &Version,
    commits: &[Commit],
) -> Result<Prepared<'r>, String> {
    let repo = &pipeline.repo;
    let target = releases.pick_target(version)?;
    repo.refs.refuse_checked_out(&target.line.branch)?;
    repo.refs
        .refuse_checked_out(&releases::notes_branch(version))?;
    let local = repo.refs.branch(&target.line.branch)?.local;
    let filing = plan(pipeline, checkout, releases, target.line, version, commits)?;

    Ok(Prepared {
        line: target.line.clone(),
        local,
        start: target.start,
        filing,
    })
}

/// Plan what the pick of `commits` onto `line` files for `version`: each
/// fragment and highlight they added, moved or copied into the release's
/// directory, on the release's notes branch with `checkout` merged in, or on a
/// new one from `checkout`.
///
/// The error says why one can't be filed, such as an upgrade section `version`
/// can't ship, or that the notes branch and `checkout` conflict.
fn plan<'r>(
    pipeline: &'r Pipeline<'_>,
    checkout: &Checkout,
    releases: &Releases,
    line: &Line,
    version: &Version,
    commits: &[Commit],
) -> Result<Filing<'r>, String> {
    let repo = &pipeline.repo;
    let branch = repo.refs.branch(&releases::notes_branch(version))?;
    let notes = repo.worktrees.add("pick", &checkout.head)?;
    if let Some(tip) = repo.refs.tip(&branch)? {
        merge_notes(&notes, &branch.name, &tip)?;
    }

    let dir = pipeline.fragments_in(&notes);
    let mut filing = Filing {
        notes,
        branch,
        files: vec![],
        gone: vec![],
    };
    if !dir.is_dir() {
        return Ok(filing);
    }

    let history = History::read(&dir).map_err(|error| error.to_string())?;
    let planner = Planner {
        pipeline,
        releases,
        line,
        version,
        history: &history,
        dir: &dir,
    };
    for commit in commits {
        // Whether another release ships the commit, asked once.
        let mut elsewhere = None;

        for added in history.added_by(&commit.hash) {
            let Some(now) = added.now else {
                filing.gone.push(format!(
                    "{} added {}/{}, which {MAIN} no longer has, so it isn't filed.",
                    short(&commit.hash),
                    pipeline.fragments,
                    added.path
                ));
                continue;
            };
            if let Some(file) = planner.file(&commit.hash, now, &mut elsewhere)? {
                filing.files.push(file);
            }
        }
    }

    Ok(filing)
}

/// What [`plan`] files the notes of a pick into `version`, onto `line`, by.
struct Planner<'a, 'p> {
    pipeline: &'a Pipeline<'p>,
    releases: &'a Releases,
    line: &'a Line,
    version: &'a Version,

    /// The history of the fragments directory along `main`'s line.
    history: &'a History,

    /// The fragments directory, in a worktree at `main` with the notes branch
    /// merged in.
    dir: &'a Utf8Path,
}

impl Planner<'_, '_> {
    /// How the fragment or highlight at `now` below the fragments directory,
    /// which the commit `hash` added, is filed into the release's directory:
    /// moved, or copied with its credits when another release ships `hash` too;
    /// `None` for a file filed there already, or that is neither.
    ///
    /// `elsewhere` keeps whether another release ships `hash`, once asked.
    /// The error says why the file can't be filed.
    fn file(
        &self,
        hash: &str,
        now: String,
        elsewhere: &mut Option<bool>,
    ) -> Result<Option<Filed>, String> {
        let fragments = &self.pipeline.fragments;
        let Some(place) = Place::parse(&now) else {
            return Ok(None);
        };
        let to = Place {
            part: Part::Release(self.version.clone()),
            highlight: place.highlight,
            name: place.name.clone(),
        };
        if place.part == to.part {
            return Ok(None);
        }
        if self.dir.join(to.path()).exists() {
            return Err(format!(
                "{fragments}/{} exists already, so {fragments}/{now} can't be filed there.",
                to.path(),
            ));
        }

        let text = fs::read_to_string(self.dir.join(&now))
            .map_err(|error| format!("failed to read {fragments}/{now}: {error}"))?;
        let listed = lists_credits(
            self.pipeline,
            self.version,
            hash,
            &now,
            &text,
            place.highlight,
        )?;
        let copy = match place.part {
            Part::Release(_) => true,
            Part::Unreleased => self.shipped_elsewhere(hash, elsewhere)?,
        };
        let copy = if !copy {
            None
        } else if listed {
            Some(text)
        } else {
            let forge = self.pipeline.forge();
            let credits = self.history.credits(&now, forge);
            Some(with_credits(&text, place.highlight, &credits, forge))
        };

        Ok(Some(Filed {
            from: now,
            to: to.path(),
            highlight: place.highlight,
            copy,
        }))
    }

    /// Whether a release besides the one on `line` ships the commit `hash`,
    /// asked of the repository once and kept in `elsewhere`.
    fn shipped_elsewhere(&self, hash: &str, elsewhere: &mut Option<bool>) -> Result<bool, String> {
        if let Some(shipped) = *elsewhere {
            return Ok(shipped);
        }

        let containing = self.pipeline.repo.history.containing(hash)?;
        let shipped = self
            .releases
            .shipping(hash, &containing)
            .iter()
            .any(|other| other.branch != self.line.branch);
        *elsewhere = Some(shipped);

        Ok(shipped)
    }
}

/// Merge the notes branch `name`, at `tip`, into `notes`, a worktree at `main`,
/// so it holds both what `main` added since the branch started and the notes
/// the branch filed.
///
/// `main` is the merge's first parent, so the history of the fragments
/// directory is read along `main`'s line.
/// A merge that conflicts is undone, and the error says how to resolve it.
fn merge_notes(notes: &Worktree<'_>, name: &str, tip: &str) -> Result<(), String> {
    let message = format!("Merge {MAIN} into {name}");
    let ran = notes.try_git(&["merge", "--no-edit", "--quiet", "--message", &message, tip])?;
    if ran.success {
        return Ok(());
    }

    let conflicts = unmerged(notes)?;
    drop(notes.try_git(&["merge", "--abort"]));
    if conflicts.is_empty() {
        return Err(format!("Merging {MAIN} into {name} failed: {}", ran.said()));
    }

    Err(format!(
        "Merging {MAIN} into {name} conflicts in {}. If {name} has merged into {MAIN}, delete it \
         with `git branch -D {name}` and `git push {ORIGIN} --delete {name}`, and pick again; \
         otherwise merge {MAIN} into it yourself, resolving the conflicts, and pick again.",
        conflicts.join(", ")
    ))
}

/// Whether the fragment or highlight `text`, at `path` below the fragments
/// directory, lists the credits to give.
///
/// The error refuses a fragment with an upgrade section, which `version` can't
/// ship unless it is a major release, or a minor one before 1.0.
fn lists_credits(
    pipeline: &Pipeline<'_>,
    version: &Version,
    hash: &str,
    path: &str,
    text: &str,
    highlight: bool,
) -> Result<bool, String> {
    let malformed = |error: String| format!("{}/{path}: {error}", pipeline.fragments);

    if highlight {
        let highlight = highlight::parse(text).map_err(|error| malformed(error.to_string()))?;
        return Ok(highlight.credits.is_some());
    }

    let name = path.rsplit('/').next().unwrap_or(path);
    let (_, type_) = fragment::name(name).map_err(|error| malformed(error.to_string()))?;
    let fragment = fragment::parse(type_, text).map_err(|error| malformed(error.to_string()))?;
    if fragment.upgrade.is_some() && !releases::can_break(version) {
        let kind = if version.patch > 0 {
            "a patch release"
        } else {
            "a minor release from 1.0 on"
        };
        return Err(format!(
            "{version} is {kind}, which can't ship a change that requires users to act, and {} \
             added {}/{path}, which has an upgrade section.",
            short(hash),
            pipeline.fragments
        ));
    }

    Ok(fragment.credits.is_some())
}

/// The message of the commit that files the notes of `commits`, picked onto
/// `branch` for `version`.
fn notes_message(
    pipeline: &Pipeline<'_>,
    version: &Version,
    branch: &str,
    commits: &[Commit],
    filing: &Filing<'_>,
) -> String {
    let first = commits.first().map_or("", |commit| short(&commit.hash));
    let title = match commits.len() {
        1 => format!("chore(release): Pick {first} into release {version}"),
        count => format!(
            "chore(release): Pick {first} and {} more into release {version}",
            count - 1
        ),
    };
    let list: Vec<String> = commits
        .iter()
        .map(|commit| format!("- {} {}", short(&commit.hash), commit.subject))
        .collect();
    let copies = if filing.files.iter().any(|file| file.copy.is_some()) {
        ", copying the ones another release ships too, with their credits"
    } else {
        ""
    };

    format!(
        "{title}\n\n`jp release pick` picked {} onto `{branch}` for release \
         {version}:\n\n{}\n\nThis files the {} they added into `{}/{version}/`{copies}.\n",
        counted(commits.len(), "commit", "commits"),
        list.join("\n"),
        filed(&filing.files),
        pipeline.fragments,
    )
}

/// How many fragments and highlights `list` holds, as [`files`] says it.
fn filed(list: &[Filed]) -> String {
    let highlights = list.iter().filter(|file| file.highlight).count();

    files(list.len() - highlights, highlights)
}

/// Cherry-pick `hash` in `code`, leaving out what it changes in the fragments
/// directory.
///
/// A conflict in the fragments directory resolves by leaving those files out.
/// A conflict anywhere else stops the pick, with the notes already left out.
fn pick_one(pipeline: &Pipeline<'_>, code: &Worktree<'_>, hash: &str) -> Result<Picked, String> {
    let parents = code.git(&["rev-list", "--parents", "-n", "1", hash])?;
    let mut args = vec!["cherry-pick", "-x", "--keep-redundant-commits"];
    if parents.split_whitespace().count() > 2 {
        args.extend(["-m", "1"]);
    }
    args.push(hash);

    let ran = code.try_git(&args)?;
    if ran.success {
        drop_notes(pipeline, code)?;
        return Ok(Picked::Done);
    }

    if unmerged(code)?.is_empty() {
        return Err(format!("Picking {} failed: {}", short(hash), ran.said()));
    }

    // A release branch keeps no notes, so only a conflict in the code is the
    // user's to resolve.
    leave_out_notes(pipeline, code)?;
    let conflicts = unmerged(code)?;
    if !conflicts.is_empty() {
        return Ok(Picked::Conflicts(conflicts));
    }
    code.git(&["cherry-pick", "--continue"])?;

    Ok(Picked::Done)
}

/// Finish the pick of `hash` a conflict stopped in `code`, once the conflict is
/// resolved.
///
/// The error says what is left to do.
fn resolve(pipeline: &Pipeline<'_>, code: &Worktree<'_>, hash: &str) -> Result<(), String> {
    let picking = code
        .try_git(&["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"])?
        .success;
    if picking {
        leave_out_notes(pipeline, code)?;
        let conflicts = unmerged(code)?;
        if !conflicts.is_empty() {
            return Err(format!(
                "{} still has conflicts in {}; resolve them, stage the result with `git add`, and \
                 run `jp release pick --continue` again.",
                code.top(),
                conflicts.join(", ")
            ));
        }
        code.git(&["cherry-pick", "--continue"])?;
        return Ok(());
    }

    // The pick was committed in the worktree, or abandoned there.
    let message = code.git(&["log", "-1", "--format=%B"])?;
    if !message.contains(&format!("(cherry picked from commit {hash})")) {
        return Err(format!(
            "The last commit in {} isn't the pick of {}; pick it there with `git cherry-pick -x \
             {hash}`, or drop the pick with `git worktree remove --force {}`.",
            code.top(),
            short(hash),
            code.top()
        ));
    }

    drop_notes(pipeline, code)
}

/// Remove the fragments directory from the index and the work tree of `code`,
/// whatever a pick staged in it.
fn leave_out_notes(pipeline: &Pipeline<'_>, code: &Worktree<'_>) -> Result<(), String> {
    code.git(&[
        "rm",
        "-r",
        "--force",
        "--quiet",
        "--ignore-unmatch",
        "--",
        &pipeline.fragments,
    ])
    .map(drop)
}

/// Leave the fragments directory out of the last commit in `code`.
fn drop_notes(pipeline: &Pipeline<'_>, code: &Worktree<'_>) -> Result<(), String> {
    leave_out_notes(pipeline, code)?;

    // `--quiet` exits 1 when the index differs from the commit.
    let unchanged = code.try_git(&["diff", "--cached", "--quiet"])?.success;
    if !unchanged {
        code.git(&["commit", "--amend", "--no-edit", "--quiet", "--allow-empty"])?;
    }

    Ok(())
}

/// The paths with unresolved conflicts in `worktree`, below the top of the work
/// tree.
fn unmerged(worktree: &Worktree<'_>) -> Result<Vec<String>, String> {
    let listing = worktree.git(&["diff", "--name-only", "--diff-filter=U", "-z"])?;

    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Keep `code` and record what the pick of `commits` into `version` has left to
/// do, from the commit at `index` whose pick stopped at a conflict in `paths`.
///
/// Returns what to tell the user.
fn stop(
    state: &Utf8Path,
    code: Worktree<'_>,
    version: &Version,
    commits: &[Commit],
    index: usize,
    paths: &[String],
) -> Result<String, String> {
    let top = code.keep();
    save(state, &Stopped {
        worktree: top.clone(),
        version: version.to_string(),
        commits: commits.iter().map(|commit| commit.hash.clone()).collect(),
        stopped: index,
    })?;

    let commit = &commits[index];
    Ok(format!(
        "Picking {} ({}) stopped at a conflict in {}. Resolve it in {top}, stage the result with \
         `git add`, and run `jp release pick --continue`.",
        short(&commit.hash),
        commit.subject,
        paths.join(", ")
    ))
}

/// Point the release branch at `picked`, the picks `prepared` planned made on
/// top of it, file the notes on the notes branch, and say what was done and how
/// to publish it.
///
/// A release branch already at `picked`, as a retry finds it, is left there.
fn finish(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    prepared: &Prepared<'_>,
    picked: &str,
    version: &Version,
    commits: &[Commit],
) -> Result<String, String> {
    update_branches(pipeline, prepared, picked, version, commits)?;

    picked_report(pipeline, checkout, prepared, version, commits)
}

/// Point the release branch at `picked`, and file the notes `prepared` planned
/// on the notes branch, when it planned any.
///
/// Refuses a release branch that moved since the pick started.
fn update_branches(
    pipeline: &Pipeline<'_>,
    prepared: &Prepared<'_>,
    picked: &str,
    version: &Version,
    commits: &[Commit],
) -> Result<(), String> {
    let repo = &pipeline.repo;
    let branch = &prepared.line.branch;
    if !repo.history.is_ancestor(&prepared.line.tip, picked)? {
        return Err(format!(
            "{branch} moved since the pick started, to {}; pick the commits again.",
            short(&prepared.line.tip)
        ));
    }
    if prepared.local.as_deref() != Some(picked) {
        repo.refs
            .set_branch(branch, picked, prepared.local.as_deref())?;
    }

    if prepared.filing.files.is_empty() {
        return Ok(());
    }
    let message = notes_message(pipeline, version, branch, commits, &prepared.filing);
    let hash = apply(pipeline, &prepared.filing, &message)?;

    repo.refs.set_branch(
        &releases::notes_branch(version),
        &hash,
        prepared.filing.branch.local.as_deref(),
    )
}

/// What the pick of `commits` into `version` did, as `prepared` planned it, and
/// how to publish it.
fn picked_report(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    prepared: &Prepared<'_>,
    version: &Version,
    commits: &[Commit],
) -> Result<String, String> {
    let branch = &prepared.line.branch;
    let notes_branch = releases::notes_branch(version);
    let filed_notes = !prepared.filing.files.is_empty();

    let picked_count = counted(commits.len(), "commit", "commits");
    let mut said = if prepared.start {
        format!("Started release {version} on {branch} with {picked_count} picked onto it.\n")
    } else {
        format!("Picked {picked_count} onto {branch} for release {version}.\n")
    };
    for gone in &prepared.filing.gone {
        said.push_str(gone);
        said.push('\n');
    }

    let mut refs = vec![branch.clone()];
    if filed_notes {
        said.push_str(&format!(
            "Filed {} into {}/{version}/ on {notes_branch}.\n",
            filed(&prepared.filing.files),
            pipeline.fragments
        ));
        refs.push(notes_branch.clone());
    } else {
        said.push_str(&format!(
            "{} added no fragment or highlight to file.\n",
            if commits.len() == 1 {
                "The commit"
            } else {
                "The commits"
            }
        ));
    }
    if let Some(ahead) = checkout.ahead() {
        let notes = filed_notes.then_some(notes_branch.as_str());
        let holding = holding(pipeline, checkout, commits, branch, notes)?;
        if !holding.is_empty() {
            said.push_str(&format!(
                "{ahead}: {} {} commits {MAIN} doesn't have yet.\n",
                listed(&holding),
                if holding.len() == 1 { "holds" } else { "hold" }
            ));
        }
    }

    said.push_str(&format!(
        "\nNothing is published yet. Publish {} with:\n\n{}",
        if refs.len() == 1 { "it" } else { "them" },
        push(&refs)
    ));
    if filed_notes {
        said.push_str(&format!(
            "\nthen merge {notes_branch} into {MAIN} once its notes are final.\n"
        ));
    }

    Ok(said)
}

/// The branches a pick of `commits` made, from a checkout ahead of `origin`'s
/// `main`, that hold commits `origin`'s `main` lacks: the release branch
/// `branch` when one of `commits` is such a commit, and the notes branch
/// `notes`, when the pick filed notes on it.
fn holding(
    pipeline: &Pipeline<'_>,
    checkout: &Checkout,
    commits: &[Commit],
    branch: &str,
    notes: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut holding = vec![];
    for commit in commits {
        if !pipeline
            .repo
            .history
            .is_ancestor(&commit.hash, &checkout.main)?
        {
            holding.push(branch.to_owned());
            break;
        }
    }
    holding.extend(notes.map(ToOwned::to_owned));

    Ok(holding)
}

/// Move or copy each file of `filing` into the release's directory, and commit
/// it with `message`.
///
/// Returns the commit's hash.
fn apply(pipeline: &Pipeline<'_>, filing: &Filing<'_>, message: &str) -> Result<String, String> {
    let workspace = filing.notes.workspace();

    for file in &filing.files {
        let from = format!("{}/{}", pipeline.fragments, file.from);
        let to = format!("{}/{}", pipeline.fragments, file.to);
        let path = workspace.join(&to);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {parent}: {error}"))?;
        }

        match &file.copy {
            Some(text) => {
                fs::write(&path, text).map_err(|error| format!("failed to write {to}: {error}"))?;
            }
            None => {
                filing.notes.git(&["mv", &from, &to])?;
            }
        }
    }

    filing.notes.commit(message)
}

/// The pick a conflict stopped, as the release it goes into and the worktree
/// holding it, when there is one to continue.
pub(crate) fn stopped(pipeline: &Pipeline<'_>) -> Result<Option<(String, Utf8PathBuf)>, String> {
    let state = pipeline.repo.worktrees.common_dir()?.join(STOPPED);

    Ok(load(&state)?
        .filter(|stopped| stopped.worktree.join(".git").exists())
        .map(|stopped| (stopped.version, stopped.worktree)))
}

/// The stopped pick `path` keeps, when there is one.
fn load(path: &Utf8Path) -> Result<Option<Stopped>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read {path}: {error}")),
    };

    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{path} doesn't hold a stopped pick ({error}); remove it."))
}

/// Keep `stopped` in `path`.
fn save(path: &Utf8Path, stopped: &Stopped) -> Result<(), String> {
    let json = serde_json::to_string_pretty(stopped)
        .map_err(|error| format!("failed to write the stopped pick: {error}"))?;

    fs::write(path, json + "\n").map_err(|error| format!("failed to write {path}: {error}"))
}

/// Forget the stopped pick `path` keeps.
fn remove(path: &Utf8Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to remove {path}: {error}")),
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
