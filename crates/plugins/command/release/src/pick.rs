//! `jp release pick`: add commits from `main` to a release in preparation, or
//! start the next patch release with them.
//!
//! The commits are cherry-picked onto the release branch with `git cherry-pick
//! -x`, in the order `main` has them, leaving out what they change in the
//! fragments directory, which a release branch doesn't have; the branch is then
//! pushed.
//! The fragments and highlights they added are filed into the release's
//! directory on `main`, through a change request from
//! `release-pick/v<version>-<short hash>`.
//! One that no other release ships moves, so the next release doesn't list the
//! change again; one another release ships too is copied, with its credits
//! written into the copy's `change_requests`, so both releases list it.
//!
//! A conflict stops the pick in its worktree, which is kept for the user to
//! resolve, and `jp release pick --continue` finishes it.
//! What the stopped pick has left to do is kept in the repository's git
//! directory, beside its worktrees.

use std::{collections::HashMap, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{History, Place, directory::Part, fragment, highlight, with_change_requests};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    change_request::{ChangeRequest, Opener},
    pipeline::{Pipeline, counted, files, short},
    releases::{self, Line, Releases},
    repository::{MAIN, Worktree},
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

/// What a pick files on `main`, planned in a worktree at its tip.
struct Filing<'r> {
    notes: Worktree<'r>,
    files: Vec<Filed>,

    /// The files the commits added that `main` no longer has, as notes for the
    /// user.
    gone: Vec<String>,
}

/// Everything a pick needs settled before it pushes.
struct Prepared<'r> {
    /// The release branch.
    line: Line,

    /// Whether the pick starts the release.
    start: bool,

    filing: Filing<'r>,

    /// The change request filing the notes, and how it is opened; `None` when
    /// there is nothing to file.
    request: Option<(ChangeRequest, Opener)>,
}

/// What became of one cherry-pick.
enum Picked {
    Done,

    /// It stopped at a conflict in these paths, below the top of the work tree.
    Conflicts(Vec<String>),
}

/// Pick `commits` from `main` into release `version`, and say what was done.
///
/// The error says why nothing was picked, where a conflict stopped the pick, or
/// what was done before a step failed.
pub(crate) fn pick(
    pipeline: &Pipeline<'_>,
    version: &Version,
    commits: &[String],
) -> Result<String, String> {
    let repo = &pipeline.repo;
    let state = repo.common_dir()?.join(STOPPED);
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

    repo.refuse_shallow()?;
    repo.fetch()?;
    let releases = repo.releases()?;
    let target = releases.pick_target(version)?;
    let commits = order(pipeline, target.line, commits)?;
    let prepared = prepare(pipeline, &releases, version, &commits)?;

    let code = repo.worktree("pick", &target.line.tip)?;
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

    repo.push(&code.head()?, &prepared.line.branch, false, &[])?;
    drop(code);

    publish(pipeline, version, &prepared, &commits)
}

/// Finish the pick a conflict stopped, once the conflict is resolved, and say
/// what was done.
///
/// The error says why it can't be finished yet; the stopped pick is kept, to
/// try again.
pub(crate) fn resume(pipeline: &Pipeline<'_>) -> Result<String, String> {
    let repo = &pipeline.repo;
    let state = repo.common_dir()?.join(STOPPED);
    let Some(stopped) = load(&state)? else {
        return Err("No pick is stopped, so there is nothing to continue.".to_owned());
    };
    let Ok(code) = repo.reopen(&stopped.worktree) else {
        remove(&state)?;
        return Err(format!(
            "The worktree the pick into {} stopped in, {}, is gone; pick the commits again.",
            stopped.version, stopped.worktree
        ));
    };
    let version = Version::parse(&stopped.version)
        .map_err(|error| format!("{state} names no version: {error}"))?;
    let mut commits = vec![];
    for hash in &stopped.commits {
        let subject = repo.git(&["log", "-1", "--format=%s", hash])?;
        commits.push(Commit {
            hash: hash.clone(),
            subject: subject.trim().to_owned(),
        });
    }

    if let Some(commit) = commits.get(stopped.stopped)
        && let Err(error) = finish(pipeline, &code, &commit.hash)
    {
        code.keep();
        return Err(error);
    }
    for (index, commit) in commits.iter().enumerate().skip(stopped.stopped + 1) {
        match pick_one(pipeline, &code, &commit.hash) {
            Ok(Picked::Done) => {}
            Ok(Picked::Conflicts(paths)) => {
                return Err(stop(&state, code, &version, &commits, index, &paths)?);
            }
            Err(error) => {
                let top = code.keep();
                save(&state, &Stopped {
                    stopped: index,
                    ..stopped
                })?;
                return Err(format!(
                    "{error}\nThe pick is kept in {top}; finish it there and run `jp release pick \
                     --continue`, or drop it with `git worktree remove --force {top}`."
                ));
            }
        }
    }

    // Every pick is made, and the worktree is kept until the release branch is
    // pushed.
    let pushed = (|| {
        repo.refuse_shallow()?;
        repo.fetch()?;
        let releases = repo.releases()?;
        let prepared = prepare(pipeline, &releases, &version, &commits)?;
        repo.push(&code.head()?, &prepared.line.branch, false, &[])?;

        Ok::<_, String>(prepared)
    })();
    let prepared = match pushed {
        Ok(prepared) => prepared,
        Err(error) => {
            let top = code.keep();
            save(&state, &Stopped {
                stopped: commits.len(),
                ..stopped
            })?;
            return Err(format!(
                "{error}\nThe picks are kept in {top}; run `jp release pick --continue` to try \
                 again, or drop them with `git worktree remove --force {top}`."
            ));
        }
    };
    remove(&state)?;
    drop(code);

    publish(pipeline, &version, &prepared, &commits)
}

/// The commits `commits` name, in the order `main` has them, each once.
///
/// The error names a commit that isn't on `main`'s first-parent line, that
/// `line` has already, or that changes nothing but notes.
fn order(pipeline: &Pipeline<'_>, line: &Line, commits: &[String]) -> Result<Vec<Commit>, String> {
    let repo = &pipeline.repo;
    let main_line = repo.main_line()?;
    let positions: HashMap<&str, usize> = main_line
        .iter()
        .enumerate()
        .map(|(position, hash)| (hash.as_str(), position))
        .collect();
    let notes = format!("{}{}/", repo.prefix(), pipeline.fragments);

    let mut ordered: Vec<(usize, Commit)> = vec![];
    for commit in commits {
        let hash = repo.commit(commit)?;
        if ordered.iter().any(|(_, known)| known.hash == hash) {
            continue;
        }
        let Some(&position) = positions.get(hash.as_str()) else {
            return Err(format!(
                "`{commit}` isn't on `{MAIN}`'s first-parent line; pick the commit that brought \
                 it to `{MAIN}`, such as the merge of its branch."
            ));
        };
        if line.picked.contains(&hash) || repo.containing(&hash)?.contains(&line.branch) {
            return Err(format!("{} has {} already.", line.branch, short(&hash)));
        }

        let changed = repo.git(&[
            "log",
            "-1",
            "-z",
            "--format=",
            "--name-only",
            "--diff-merges=first-parent",
            &hash,
        ])?;
        let code = changed
            .split('\0')
            .map(|path| path.trim_start_matches('\n'))
            .any(|path| !path.is_empty() && !path.starts_with(&notes));
        if !code {
            return Err(format!(
                "{} changes nothing outside {}, and release notes are corrected on `{MAIN}`, not \
                 picked.",
                short(&hash),
                pipeline.fragments
            ));
        }

        let subject = repo.git(&["log", "-1", "--format=%s", &hash])?;
        ordered.push((position, Commit {
            hash,
            subject: subject.trim().to_owned(),
        }));
    }

    // `main_line` lists the newest commit first.
    ordered.sort_by(|(a, _), (b, _)| b.cmp(a));

    Ok(ordered.into_iter().map(|(_, commit)| commit).collect())
}

/// Settle where the pick of `commits` into `version` goes, what it files on
/// `main`, and how its change request is opened.
fn prepare<'r>(
    pipeline: &'r Pipeline<'_>,
    releases: &Releases,
    version: &Version,
    commits: &[Commit],
) -> Result<Prepared<'r>, String> {
    let target = releases.pick_target(version)?;
    let filing = plan(pipeline, releases, target.line, version, commits)?;

    let request = if filing.files.is_empty() {
        None
    } else {
        let request = change_request(pipeline, version, &target.line.branch, commits, &filing);
        if pipeline.repo.has_branch(&request.branch)? {
            return Err(format!(
                "{} exists on origin already; delete it if an earlier pick left it behind.",
                request.branch
            ));
        }
        Some((request, pipeline.opener()?))
    };

    Ok(Prepared {
        line: target.line.clone(),
        start: target.start,
        filing,
        request,
    })
}

/// Plan what the pick of `commits` onto `line` files on `main` for `version`:
/// each fragment and highlight they added, moved or copied into the release's
/// directory.
///
/// The error says why one can't be filed, such as an upgrade section `version`
/// can't ship.
fn plan<'r>(
    pipeline: &'r Pipeline<'_>,
    releases: &Releases,
    line: &Line,
    version: &Version,
    commits: &[Commit],
) -> Result<Filing<'r>, String> {
    let repo = &pipeline.repo;
    let notes = repo.worktree("pick", &repo.main()?)?;
    let dir = pipeline.fragments_in(&notes);
    let mut filing = Filing {
        notes,
        files: vec![],
        gone: vec![],
    };
    if !dir.is_dir() {
        return Ok(filing);
    }

    let history = History::read(&dir).map_err(|error| error.to_string())?;
    let forge = pipeline.forge();
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
            let Some(place) = Place::parse(&now) else {
                continue;
            };
            let to = Place {
                part: Part::Release(version.clone()),
                highlight: place.highlight,
                name: place.name.clone(),
            };
            if place.part == to.part {
                continue;
            }
            if dir.join(to.path()).exists() {
                return Err(format!(
                    "{}/{} exists already, so {}/{now} can't be filed there.",
                    pipeline.fragments,
                    to.path(),
                    pipeline.fragments
                ));
            }

            let text = fs::read_to_string(dir.join(&now))
                .map_err(|error| format!("failed to read {}/{now}: {error}", pipeline.fragments))?;
            let listed = lists_credits(
                pipeline,
                version,
                &commit.hash,
                &now,
                &text,
                place.highlight,
            )?;

            let copy = match (&place.part, elsewhere) {
                (Part::Release(_), _) => true,
                (Part::Unreleased, Some(shipped)) => shipped,
                (Part::Unreleased, None) => {
                    let containing = repo.containing(&commit.hash)?;
                    let shipped = releases
                        .shipping(&commit.hash, &containing)
                        .iter()
                        .any(|other| other.branch != line.branch);
                    elsewhere = Some(shipped);
                    shipped
                }
            };
            let copy = if !copy {
                None
            } else if listed {
                Some(text)
            } else {
                let numbers = history.change_requests(&now, forge);
                if numbers.is_empty() {
                    Some(text)
                } else {
                    Some(
                        with_change_requests(&text, &numbers)
                            .map_err(|error| format!("{}/{now}: {error}", pipeline.fragments))?,
                    )
                }
            };

            filing.files.push(Filed {
                from: now,
                to: to.path(),
                highlight: place.highlight,
                copy,
            });
        }
    }

    Ok(filing)
}

/// Whether the fragment or highlight `text`, at `path` below the fragments
/// directory, lists the change requests to credit.
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
        return Ok(highlight.change_requests.is_some());
    }

    let fragment = fragment::parse(text).map_err(|error| malformed(error.to_string()))?;
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

    Ok(fragment.change_requests.is_some())
}

/// The change request that files the notes of `commits`, picked onto `branch`
/// for `version`.
fn change_request(
    pipeline: &Pipeline<'_>,
    version: &Version,
    branch: &str,
    commits: &[Commit],
    filing: &Filing<'_>,
) -> ChangeRequest {
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

    ChangeRequest {
        branch: format!("release-pick/v{version}-{first}"),
        title,
        body: format!(
            "`jp release pick` picked {} onto `{branch}` for release {version}:\n\n{}\n\nThis \
             files the {} they added into `{}/{version}/`{copies}.\n",
            counted(commits.len(), "commit", "commits"),
            list.join("\n"),
            filed(&filing.files),
            pipeline.fragments,
        ),
    }
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
fn finish(pipeline: &Pipeline<'_>, code: &Worktree<'_>, hash: &str) -> Result<(), String> {
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

/// The paths with unresolved conflicts in `code`, below the top of the work
/// tree.
fn unmerged(code: &Worktree<'_>) -> Result<Vec<String>, String> {
    let listing = code.git(&["diff", "--name-only", "--diff-filter=U", "-z"])?;

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

/// File the notes of a pick that pushed its release branch, and say what was
/// done.
///
/// The error says what was done before a step failed.
fn publish(
    pipeline: &Pipeline<'_>,
    version: &Version,
    prepared: &Prepared<'_>,
    commits: &[Commit],
) -> Result<String, String> {
    let picked = counted(commits.len(), "commit", "commits");
    let branch = &prepared.line.branch;
    let mut said = if prepared.start {
        format!(
            "Started release {version} on {branch} with {picked} picked onto it, and pushed it.\n"
        )
    } else {
        format!("Picked {picked} onto {branch} for release {version}, and pushed it.\n")
    };
    for gone in &prepared.filing.gone {
        said.push_str(gone);
        said.push('\n');
    }

    let Some((request, opener)) = &prepared.request else {
        said.push_str(&format!(
            "{} added no fragment or highlight to file on {MAIN}.\n",
            if commits.len() == 1 {
                "The commit"
            } else {
                "The commits"
            }
        ));
        return Ok(said);
    };

    let filed_notes = apply(pipeline, &prepared.filing, request).and_then(|hash| {
        pipeline
            .repo
            .push(&hash, &request.branch, true, &opener.push_options(request))
    });
    let pushed = match filed_notes {
        Ok(pushed) => pushed,
        Err(error) => return Err(format!("{said}But its notes weren't filed: {error}")),
    };
    let opened = match opener.open(request, &pushed) {
        Ok(opened) => opened,
        Err(error) => {
            return Err(format!(
                "{said}Pushed {}, but its change request wasn't opened: {error}",
                request.branch
            ));
        }
    };

    said.push_str(&format!(
        "Pushed {}, which files {} into {}/{version}/.\n{opened}.\n",
        request.branch,
        filed(&prepared.filing.files),
        pipeline.fragments
    ));
    Ok(said)
}

/// Move or copy each file of `filing` into the release's directory, and commit
/// it as `request` describes.
///
/// Returns the commit's hash.
fn apply(
    pipeline: &Pipeline<'_>,
    filing: &Filing<'_>,
    request: &ChangeRequest,
) -> Result<String, String> {
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

    filing
        .notes
        .commit(&format!("{}\n\n{}", request.title, request.body))
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
