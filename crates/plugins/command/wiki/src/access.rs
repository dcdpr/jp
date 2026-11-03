//! What a command may read and write.
//!
//! A command the user runs reaches every file the workspace holds.
//! A tool call is held to the access policy the host compiled for it, file by
//! file: being allowed to read a page grants nothing about the files it embeds
//! or links to, and being allowed to edit a page grants nothing about the files
//! a rename rewrites.
//! Every file a tool reads or writes is asked of [`Access`] first.
//! What git holds about a file, which pinning and checking a permalink ask git
//! for, is read only for a file the access lets the command read.

use std::path::MAIN_SEPARATOR;

use camino::Utf8Path;
#[cfg(test)]
use jp_tool::{AccessPolicy, Action, FsRule};
use jp_tool::{Capability, Context, FsAccessError};

/// What a command may read and write.
#[derive(Debug)]
pub(crate) enum Access {
    /// A command the user runs: everything the workspace holds, and anything
    /// the user names outside it.
    Unrestricted,

    /// A tool call, held to its access policy and to the workspace.
    Policy(Context),
}

impl Access {
    /// Refuse unless `capability` is allowed on `path`, an absolute path.
    ///
    /// Under a policy, a path outside the workspace is refused whatever the
    /// policy says.
    /// The refusal is the policy's own words, with paths written with `/` on
    /// every platform:
    ///
    /// ```text
    /// access denied: read on docs/rfd/042-x.md
    /// ```
    pub(crate) fn check(&self, capability: Capability, path: &Utf8Path) -> Result<(), String> {
        let Self::Policy(context) = self else {
            return Ok(());
        };

        // A refusal reaches the assistant beside paths the plugin writes with
        // `/`, so it is written the same way.
        let slashed = |text: String| text.replace(MAIN_SEPARATOR, "/");
        let relative = path.strip_prefix(&context.root).map_err(|_| {
            slashed(format!(
                "{path} is outside the workspace, which is all a tool reaches"
            ))
        })?;
        // The workspace root is `.` to the policy, the way its rules write it.
        let relative = if relative.as_str().is_empty() {
            Utf8Path::new(".")
        } else {
            relative
        };

        ask(context, relative, capability).map_err(|error| slashed(error.to_string()))
    }

    /// Whether `capability` is allowed on `path`.
    #[must_use]
    pub(crate) fn allows(&self, capability: Capability, path: &Utf8Path) -> bool {
        self.check(capability, path).is_ok()
    }

    /// Whether the contents of `path` may be read.
    #[must_use]
    pub(crate) fn reads(&self, path: &Utf8Path) -> bool {
        self.allows(Capability::Read, path)
    }

    /// Whether the workspace root may be read, which a search through the whole
    /// workspace starts with.
    #[must_use]
    pub(crate) fn reads_workspace(&self) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Policy(context) => context.check_read(Utf8Path::new(".")).is_ok(),
        }
    }

    /// Whether a policy holds the command, which a file outside the workspace
    /// is never read under.
    #[must_use]
    pub(crate) fn is_restricted(&self) -> bool {
        matches!(self, Self::Policy(_))
    }
}

#[cfg(test)]
impl Access {
    /// A tool call's access in the workspace at `root`, granting what `rules`
    /// grant and nothing else.
    pub(crate) fn granting(root: &Utf8Path, rules: Vec<FsRule>) -> Self {
        Self::Policy(Context {
            root: root.to_owned(),
            action: Action::Run,
            access: Some(AccessPolicy {
                fs: rules,
                ..AccessPolicy::default()
            }),
            workspace_id: "test".to_owned(),
            conversation_id: "test".to_owned(),
        })
    }
}

/// Ask the policy about one capability on the workspace-relative `path`.
fn ask(context: &Context, path: &Utf8Path, capability: Capability) -> Result<(), FsAccessError> {
    match capability {
        Capability::Read => context.check_read(path),
        Capability::Create => context.check_create(path),
        Capability::Update => context.check_update(path),
        Capability::Delete => context.check_delete(path),
        Capability::Execute => context.check_execute(path),
    }
    .map(drop)
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
