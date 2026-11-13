//! Fetching from `origin` and comparing the current checkout against
//! `origin/main`.

use doc::repository::web_address;

use super::{Checkout, MAIN, MAIN_REF, ORIGIN, ORIGIN_REFS, git::Git};
use crate::releases::{NOTES_BRANCH, RELEASE_BRANCH};

pub(crate) struct Origin<'a> {
    git: Git<'a>,
}

impl<'a> Origin<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    pub(crate) fn refuse_shallow(&self) -> Result<(), String> {
        let shallow = self.git.git(&["rev-parse", "--is-shallow-repository"])?;
        if shallow.trim() == "true" {
            return Err(
                "The repository is a shallow clone, which lacks the history releases are read \
                 from; fetch the rest of it with `git fetch --unshallow`."
                    .to_owned(),
            );
        }

        Ok(())
    }

    /// Fetch `main`, `release/*`, and `release-notes/*` from `origin` into
    /// remote-tracking refs, pruning deleted remote branches.
    pub(crate) fn fetch(&self) -> Result<(), String> {
        let main = format!("+refs/heads/{MAIN}:{MAIN_REF}");
        let releases = format!("+refs/heads/{RELEASE_BRANCH}*:{ORIGIN_REFS}{RELEASE_BRANCH}*");
        let notes = format!("+refs/heads/{NOTES_BRANCH}*:{ORIGIN_REFS}{NOTES_BRANCH}*");

        self.git
            .git(&[
                "fetch",
                "--quiet",
                "--prune",
                "--no-tags",
                ORIGIN,
                &main,
                &releases,
                &notes,
            ])
            .map(drop)
    }

    pub(crate) fn main(&self) -> Result<String, String> {
        self.git.commit(MAIN_REF)
    }

    pub(crate) fn address(&self) -> Option<String> {
        let url = self.git.git(&["remote", "get-url", ORIGIN]).ok()?;

        web_address(url.trim())
    }

    /// Return the current [`Checkout`], rejecting a checkout that does not
    /// contain `origin/main`.
    pub(crate) fn checkout(&self) -> Result<Checkout, String> {
        let checkout = self.checked_out()?;
        if self.lacks_main(&checkout)? {
            return Err(checkout.behind());
        }

        Ok(checkout)
    }

    /// Return the current [`Checkout`] without requiring it to contain
    /// `origin/main`.
    pub(crate) fn checked_out(&self) -> Result<Checkout, String> {
        let head = self.git.commit("HEAD").map_err(|_| {
            "The checkout has no commit yet, so there is nothing to release.".to_owned()
        })?;
        let branch = self
            .git
            .try_git(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let branch = branch.success.then(|| branch.stdout.trim().to_owned());
        let main = self.main()?;

        Ok(Checkout { head, branch, main })
    }

    pub(crate) fn lacks_main(&self, checkout: &Checkout) -> Result<bool, String> {
        Ok(!self.git.is_ancestor(&checkout.main, &checkout.head)?)
    }
}
