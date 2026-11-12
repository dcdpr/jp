//! What `cut`, `pick`, and `finalize` share: the repository they work in, the
//! plugin's options, and how they say what to publish.

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Forge, directory::Problem};

use crate::{
    options::Options,
    repository::{ORIGIN, Repository, Worktree},
    shell::Shell,
};

/// The context a release command runs in.
pub(crate) struct Pipeline<'a> {
    pub repo: Repository<'a>,
    pub options: &'a Options,

    /// What runs the project's commands.
    pub shell: &'a Shell,

    /// The workspace root, in the checkout the command runs from.
    pub root: Utf8PathBuf,

    /// The fragments directory, below the workspace root, with `/` between its
    /// parts.
    pub fragments: String,
}

impl<'a> Pipeline<'a> {
    /// The context for the workspace at `root`, running programs with `shell`.
    ///
    /// The error says why the workspace can't make releases.
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

    /// The fragments directory in `worktree`.
    pub(crate) fn fragments_in(&self, worktree: &Worktree<'_>) -> Utf8PathBuf {
        worktree.workspace().join(&self.fragments)
    }

    /// The forge change requests are reviewed on: the configured one, or the
    /// one the project's [`address`] is on.
    ///
    /// [`address`]: Self::address
    pub(crate) fn forge(&self) -> Forge {
        self.options
            .notes
            .forge
            .unwrap_or_else(|| Forge::infer(self.address().as_deref()))
    }

    /// The project's web address, which credits link from: the configured
    /// `repository`, or `origin`'s.
    pub(crate) fn address(&self) -> Option<String> {
        self.options
            .notes
            .repository
            .clone()
            .or_else(|| self.repo.origin.address())
    }
}

/// The command that publishes the branches and tags `refs` to `origin`,
/// indented to stand on a line of its own.
pub(crate) fn push(refs: &[String]) -> String {
    format!("    git push {ORIGIN} {}\n", refs.join(" "))
}

/// `found` in the fragments directory of `worktree`, one problem per line, each
/// with its path below the workspace root.
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

/// `dir` below the workspace `root`, with `/` between its parts.
///
/// The error says `dir` is outside the workspace.
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

/// `path` with `/` between its parts on every platform.
pub(crate) fn slashed(path: &Utf8Path) -> String {
    path.components()
        .map(|component| component.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// `items` as a sentence lists them: `a`, `a and b`, or `a, b, and c`.
pub(crate) fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// `count` followed by the noun it counts.
pub(crate) fn counted(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{count} {noun}")
}

/// How many fragments and highlights a release files, such as `3 fragments and
/// 1 highlight`, leaving out a kind it files none of.
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

/// The first seven characters of the commit hash `hash`.
pub(crate) fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
