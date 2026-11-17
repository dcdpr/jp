//! The merge queue as this window sees it: whether it is paused here, and the
//! label edits sent to GitHub that no poll has shown yet.

use std::collections::HashMap;

use crate::model::PullRequest;

#[derive(Debug, Default)]
pub struct QueueView {
    /// Whether the merge queue is paused in this process.
    pub paused: bool,

    /// Pending queue-label edits not yet reflected in the polled pull request
    /// list, keyed by PR number (`true` for add, `false` for remove).
    pub edits: HashMap<u64, bool>,
}

impl QueueView {
    /// Pause the queue, or resume it, and return the notice saying which.
    pub(super) fn toggle_pause(&mut self) -> String {
        self.paused = !self.paused;
        if self.paused {
            "Merge queue paused: nothing is updated or merged until you press p again.".to_owned()
        } else {
            "Merge queue resumed.".to_owned()
        }
    }

    /// Record a toggle of pull request `number`'s queue label, which GitHub
    /// last reported as `labeled`, and return whether it adds the label.
    ///
    /// A pending edit counts as done, so a second toggle before GitHub answers
    /// undoes the first.
    pub(super) fn toggle(&mut self, number: u64, labeled: bool) -> bool {
        let labeled = self.edits.get(&number).copied().unwrap_or(labeled);
        let add = !labeled;
        self.edits.insert(number, add);
        add
    }

    /// Return the notice for a finished label edit on pull request `number`.
    ///
    /// A failed edit is forgotten, unless a later toggle replaced it.
    pub(super) fn edited(&mut self, number: u64, add: bool, outcome: Result<(), String>) -> String {
        match outcome {
            Ok(()) if add => format!("Added #{number} to the merge queue."),
            Ok(()) => format!("Took #{number} out of the merge queue."),
            Err(error) => {
                if self.edits.get(&number) == Some(&add) {
                    self.edits.remove(&number);
                }
                error
            }
        }
    }

    /// Forget the edits `prs` shows done, or whose pull request closed.
    pub(super) fn settle(&mut self, prs: &[PullRequest]) {
        self.edits.retain(|number, add| {
            prs.iter()
                .any(|pr| pr.number == *number && pr.labeled != *add)
        });
    }
}
