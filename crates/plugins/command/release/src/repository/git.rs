//! Shared `git` runner and workspace path prefix for [`super::Repository`]
//! views.

use camino::{Utf8Path, Utf8PathBuf};

use crate::shell::{Ran, Shell};

#[derive(Clone)]
pub(super) struct Git<'a> {
    pub(super) shell: &'a Shell,
    pub(super) root: Utf8PathBuf,

    /// Relative prefix from the git work-tree top level to `root` (empty when
    /// `root` is the top of the work tree, otherwise ending with `/`).
    pub(super) prefix: String,
}

impl<'a> Git<'a> {
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

    pub(super) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.shell.git(&self.root, args)
    }

    pub(super) fn try_git(&self, args: &[&str]) -> Result<Ran, String> {
        self.shell.try_git(&self.root, args)
    }

    /// Resolve `revision` to a full commit hash (`<revision>^{commit}`).
    pub(super) fn commit(&self, revision: &str) -> Result<String, String> {
        let peeled = format!("{revision}^{{commit}}");
        let ran = self.try_git(&["rev-parse", "--verify", "--quiet", &peeled])?;
        if !ran.success {
            return Err(format!("`{revision}` names no commit."));
        }

        Ok(ran.stdout.trim().to_owned())
    }

    /// Resolve `reference` to its object hash if it exists.
    pub(super) fn hash(&self, reference: &str) -> Result<Option<String>, String> {
        let ran = self.try_git(&["rev-parse", "--verify", "--quiet", reference])?;

        Ok(ran.success.then(|| ran.stdout.trim().to_owned()))
    }

    /// Return whether `ancestor` is an ancestor of (or equal to) `descendant`.
    pub(super) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let ran = self.try_git(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        // It exits 1, saying nothing, for a commit that isn't an ancestor.
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git merge-base` failed: {}", ran.said()))
    }
}
