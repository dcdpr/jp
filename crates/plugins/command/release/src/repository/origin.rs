//! `origin`, and the checkout the command runs from measured against it.

use doc::repository::web_address;

use super::{Checkout, MAIN, MAIN_REF, ORIGIN, ORIGIN_REFS, git::Git};
use crate::releases::{NOTES_BRANCH, RELEASE_BRANCH};

/// `origin`, and the checkout the command runs from measured against it.
pub(crate) struct Origin<'a> {
    git: Git<'a>,
}

impl<'a> Origin<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// Refuse a shallow clone, which lacks the history credits and shipped
    /// changes are read from.
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

    /// Fetch `main`, the release branches, and the notes branches from `origin`
    /// into their remote-tracking branches, dropping the branches `origin` no
    /// longer has.
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

    /// The hash of `origin`'s `main`, as last fetched.
    pub(crate) fn main(&self) -> Result<String, String> {
        self.git.commit(MAIN_REF)
    }

    /// The web address of `origin`, when it has one.
    pub(crate) fn address(&self) -> Option<String> {
        let url = self.git.git(&["remote", "get-url", ORIGIN]).ok()?;

        web_address(url.trim())
    }

    /// The checkout's commit, which stands for `main`.
    ///
    /// Refuses a checkout without the commits of `origin`'s `main`, as last
    /// fetched: what is made from it would leave out work `main` has.
    pub(crate) fn checkout(&self) -> Result<Checkout, String> {
        let checkout = self.checked_out()?;
        if self.lacks_main(&checkout)? {
            return Err(checkout.behind());
        }

        Ok(checkout)
    }

    /// The checkout's commit, whether or not it has the commits of `origin`'s
    /// `main`.
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

    /// Whether `checkout` lacks commits of `origin`'s `main`, as last fetched.
    pub(crate) fn lacks_main(&self, checkout: &Checkout) -> Result<bool, String> {
        Ok(!self.git.is_ancestor(&checkout.main, &checkout.head)?)
    }
}
