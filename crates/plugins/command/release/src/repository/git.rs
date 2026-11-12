//! How git runs on the repository, which every view of it shares.

use camino::{Utf8Path, Utf8PathBuf};

use crate::shell::{Ran, Shell};

/// How git runs on the repository: in the workspace root of the checkout the
/// command runs from.
#[derive(Clone)]
pub(super) struct Git<'a> {
    pub(super) shell: &'a Shell,

    /// The workspace root, where commands that concern the whole repository
    /// run.
    pub(super) root: Utf8PathBuf,

    /// The path from the top of the work tree to the workspace root: empty, or
    /// ending in `/`.
    ///
    /// Only the views translate paths with it, so no caller of the repository
    /// sees git's paths from the top of the work tree.
    pub(super) prefix: String,
}

impl<'a> Git<'a> {
    /// Git for the repository holding the workspace at `root`.
    ///
    /// The error says the workspace isn't in a git repository.
    pub(super) fn open(shell: &'a Shell, root: &Utf8Path) -> Result<Self, String> {
        let ran = shell.try_git(root, &["rev-parse", "--show-prefix"])?;
        if !ran.success {
            return Err(format!(
                "The workspace at {root} isn't in a git repository: {}",
                ran.said()
            ));
        }

        Ok(Self {
            shell,
            root: root.to_owned(),
            prefix: ran.stdout.trim().to_owned(),
        })
    }

    /// Run git with `args` in the workspace root.
    pub(super) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.shell.git(&self.root, args)
    }

    /// Run git with `args` in the workspace root, whatever it exits with.
    pub(super) fn try_git(&self, args: &[&str]) -> Result<Ran, String> {
        self.shell.try_git(&self.root, args)
    }

    /// The full hash of the commit `revision` names.
    ///
    /// The error says it names no commit.
    pub(super) fn commit(&self, revision: &str) -> Result<String, String> {
        let peeled = format!("{revision}^{{commit}}");
        let ran = self.try_git(&["rev-parse", "--verify", "--quiet", &peeled])?;
        if !ran.success {
            return Err(format!("`{revision}` names no commit."));
        }

        Ok(ran.stdout.trim().to_owned())
    }

    /// The hash of the object the reference `reference` names, when it names
    /// one.
    pub(super) fn hash(&self, reference: &str) -> Result<Option<String>, String> {
        let ran = self.try_git(&["rev-parse", "--verify", "--quiet", reference])?;

        Ok(ran.success.then(|| ran.stdout.trim().to_owned()))
    }

    /// Whether the commit `ancestor` is in the history of `descendant`, or is
    /// it.
    pub(super) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let ran = self.try_git(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        // It exits 1, saying nothing, for a commit that isn't an ancestor.
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git merge-base` failed: {}", ran.said()))
    }
}
