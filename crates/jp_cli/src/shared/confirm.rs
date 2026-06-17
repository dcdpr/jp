//! Shared confirmation-prompt flag for mutating conversation commands.
//!
//! Exposes `--confirm`, `--no-confirm`, and the `--yes` / `-y` alias.
//! With no flag, the decision is left to the command's own default.

/// Confirmation-prompt preference shared by mutating commands.
///
/// `--confirm` forces a prompt before each change; `--no-confirm` (alias
/// `--yes`, `-y`) skips it.
/// With neither flag, [`Self::preference`] returns `None` and the command
/// applies its own default.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
pub(crate) struct ConfirmFlag {
    /// Prompt for confirmation before each change.
    #[arg(long, overrides_with = "no_confirm")]
    confirm: bool,

    /// Skip confirmation prompts.
    #[arg(
        long = "no-confirm",
        visible_alias = "yes",
        short = 'y',
        overrides_with = "confirm"
    )]
    no_confirm: bool,
}

impl ConfirmFlag {
    /// The user's explicit preference, or `None` when no confirm flag was
    /// passed.
    ///
    /// `Some(true)` always prompts, `Some(false)` never prompts, and `None`
    /// defers to the command's default.
    /// When both flags appear, the last one on the command line wins.
    pub(crate) fn preference(self) -> Option<bool> {
        if self.confirm {
            Some(true)
        } else if self.no_confirm {
            Some(false)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "confirm_tests.rs"]
mod tests;
