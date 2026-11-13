//! Shared execution context and formatting helpers for release pipeline
//! subcommands (`cut`, `pick`, `finalize`, `build`, `abandon`, `due`,
//! `status`).

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Forge, directory::Problem};

use crate::{
    options::Options,
    repository::{ORIGIN, Repository, Worktree},
    shell::Shell,
};

/// Execution context for release pipeline subcommands.
pub(crate) struct Pipeline<'a> {
    pub repo: Repository<'a>,
    pub options: &'a Options,
    pub shell: &'a Shell,
    pub root: Utf8PathBuf,

    /// `/`-separated fragments directory path relative to `root`.
    pub fragments: String,
}

impl<'a> Pipeline<'a> {
    pub(crate) fn new(
        shell: &'a Shell,
        root: &Utf8Path,
        options: &'a Options,
    ) -> Result<Self, String> {
        let repo = Repository::open(shell, root)?;
        let fragments = below(root, &options.notes.fragments_dir)?;

        Ok(Self {
            repo,
            options,
            shell,
            root: root.to_owned(),
            fragments,
        })
    }

    pub(crate) fn fragments_in(&self, worktree: &Worktree<'_>) -> Utf8PathBuf {
        worktree.workspace().join(&self.fragments)
    }

    /// Return the configured [`Forge`], or infer it from [`Self::address`].
    pub(crate) fn forge(&self) -> Forge {
        self.options
            .notes
            .forge
            .unwrap_or_else(|| Forge::infer(self.address().as_deref()))
    }

    /// Return the configured repository web URL, or derive it from git's
    /// `origin` remote.
    pub(crate) fn address(&self) -> Option<String> {
        self.options
            .notes
            .repository
            .clone()
            .or_else(|| self.repo.origin.address())
    }
}

pub(crate) fn push(refs: &[String]) -> String {
    format!("    git push {ORIGIN} {}\n", refs.join(" "))
}

pub(crate) fn problems(worktree: &Worktree<'_>, found: &[Problem]) -> String {
    let workspace = worktree.workspace();

    found
        .iter()
        .map(|problem| {
            let path = problem
                .path
                .strip_prefix(&workspace)
                .unwrap_or(&problem.path);
            format!("{}: {}", slashed(path), problem.kind)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn below(root: &Utf8Path, dir: &Utf8Path) -> Result<String, String> {
    let relative = if dir.is_absolute() {
        dir.strip_prefix(root).map_err(|_| {
            format!("The fragments directory {dir} is outside the workspace, so no release has it.")
        })?
    } else {
        dir
    };

    Ok(slashed(relative))
}

pub(crate) fn slashed(path: &Utf8Path) -> String {
    path.components()
        .map(|component| component.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

pub(crate) fn counted(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{count} {noun}")
}

pub(crate) fn files(fragments: usize, highlights: usize) -> String {
    match (fragments, highlights) {
        (_, 0) => counted(fragments, "fragment", "fragments"),
        (0, _) => counted(highlights, "highlight", "highlights"),
        _ => format!(
            "{} and {}",
            counted(fragments, "fragment", "fragments"),
            counted(highlights, "highlight", "highlights")
        ),
    }
}

pub(crate) fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
