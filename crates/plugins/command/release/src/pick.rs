//! `jp release pick`: cherry-picking commits from `main` into a release in
//! preparation or starting the next patch release.
//!
//! Picks commits from the first-parent history of the current checkout onto the
//! target release branch (`release/<major>.<minor>`) using `git cherry-pick -x`
//! in chronological order, stripping any changes inside the fragments
//! directory.
//!
//! Fragments and highlights added by the picked commits are filed into
//! `<fragments_dir>/<version>/` on `release-notes/v<version>` (merging `main`
//! into an existing notes branch first, or branching a new one from `main`).
//! If no other release branch ships the commit, the file is moved; if another
//! release branch also ships the commit, the file is copied with explicit
//! `Credits` fields so both releases list the change.
//!
//! If a code conflict occurs during cherry-picking, the worktree is preserved
//! and state is saved to `.git/jp-release-pick.json` so the user can resolve
//! the conflict and run `jp release pick --continue`.

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

/// State filename in the repository's common git directory for a paused pick.
const STOPPED: &str = "jp-release-pick.json";

/// Persisted state for a pick paused by a merge conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stopped {
    worktree: Utf8PathBuf,
    version: String,
    commits: Vec<String>,

    /// Index in `commits` of the conflicting commit, or `commits.len()` once
    /// all cherry-picks have completed.
    stopped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Commit {
    hash: String,
    subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Filed {
    from: String,
    to: String,
    highlight: bool,

    /// File contents when copying a fragment/highlight also shipped by another
    /// release, or `None` when moving the file.
    copy: Option<String>,
}

struct Filing<'r> {
    notes: Worktree<'r>,
    branch: Branch,
    files: Vec<Filed>,
    gone: Vec<String>,
}

struct Prepared<'r> {
    line: Line,
    local: Option<String>,
    start: bool,
    filing: Filing<'r>,
}

enum Picked {
    Done,
    Conflicts(Vec<String>),
}

/// Cherry-pick `commits` from `main` into release `version` and file their
/// fragments on `release-notes/v<version>`.
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

/// Resume a pick paused by a conflict after the user has staged the resolution.
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

/// Resolve and deduplicate `commits` in chronological order along the
/// first-parent history of `checkout`, rejecting commits not on `main`, commits
/// already on `line`, or commits that only touch the fragments directory.
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

    ordered.sort_by(|(a, _), (b, _)| b.cmp(a));

    Ok(ordered.into_iter().map(|(_, commit)| commit).collect())
}

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

struct Planner<'a, 'p> {
    pipeline: &'a Pipeline<'p>,
    releases: &'a Releases,
    line: &'a Line,
    version: &'a Version,
    history: &'a History,
    dir: &'a Utf8Path,
}

impl Planner<'_, '_> {
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

/// Merge `tip` (from `name`) into the `notes` worktree (checked out at `main`),
/// keeping `main` as the first parent so [`History`] walks `main`'s
/// first-parent line.
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

fn filed(list: &[Filed]) -> String {
    let highlights = list.iter().filter(|file| file.highlight).count();

    files(list.len() - highlights, highlights)
}

/// Cherry-pick `hash` in `code`, stripping any changes to the fragments
/// directory.
///
/// Conflicts inside the fragments directory are resolved automatically by
/// removing the fragments directory; conflicts elsewhere return
/// [`Picked::Conflicts`].
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

    leave_out_notes(pipeline, code)?;
    let conflicts = unmerged(code)?;
    if !conflicts.is_empty() {
        return Ok(Picked::Conflicts(conflicts));
    }
    code.git(&["cherry-pick", "--continue"])?;

    Ok(Picked::Done)
}

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

fn drop_notes(pipeline: &Pipeline<'_>, code: &Worktree<'_>) -> Result<(), String> {
    leave_out_notes(pipeline, code)?;

    let unchanged = code.try_git(&["diff", "--cached", "--quiet"])?.success;
    if !unchanged {
        code.git(&["commit", "--amend", "--no-edit", "--quiet", "--allow-empty"])?;
    }

    Ok(())
}

fn unmerged(worktree: &Worktree<'_>) -> Result<Vec<String>, String> {
    let listing = worktree.git(&["diff", "--name-only", "--diff-filter=U", "-z"])?;

    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

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

/// Return `(version, worktree_path)` if a paused pick is currently active.
pub(crate) fn stopped(pipeline: &Pipeline<'_>) -> Result<Option<(String, Utf8PathBuf)>, String> {
    let state = pipeline.repo.worktrees.common_dir()?.join(STOPPED);

    Ok(load(&state)?
        .filter(|stopped| stopped.worktree.join(".git").exists())
        .map(|stopped| (stopped.version, stopped.worktree)))
}

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

fn save(path: &Utf8Path, stopped: &Stopped) -> Result<(), String> {
    let json = serde_json::to_string_pretty(stopped)
        .map_err(|error| format!("failed to write the stopped pick: {error}"))?;

    fs::write(path, json + "\n").map_err(|error| format!("failed to write {path}: {error}"))
}

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
