//! Which operations run, in what order, and how their results fold back.
//!
//! One provider tool call becomes one [`ExecutorGroup`], which holds one
//! operation without fan-out and several with it.
//! The execution loop works in flat local indices, one per operation, so this
//! type owns the translation: it hands out the operations that may start,
//! records which never will, and folds each call's operations back into the one
//! response the provider is waiting for.
//!
//! A call without fan-out passes through unchanged: one operation, started
//! immediately, folded to its own response with no framing added.

use jp_conversation::event::ToolCallResponse;
use jp_llm::tool::{
    executor::Executor,
    fan_out::{self, OperationOutcome},
};

use super::coordinator::{ExecutorGroup, GroupOp};

/// One call's operations, as flat local indices into the execution loop's
/// bookkeeping.
struct Group {
    /// The plan index the folded response is paired with on output.
    plan_index: usize,

    /// The tool call id the folded response answers to.
    tool_id: String,

    /// The tool being called, for messages that name it.
    tool_name: String,

    /// Maximum operations of this call in flight at once, or `None` for
    /// unbounded.
    concurrency: Option<usize>,

    /// Whether a failure stops the operations that have not started.
    stop_on_error: bool,

    /// Whether this call's response needs per-operation framing.
    ///
    /// False for a call without fan-out, whose single result is its whole
    /// response.
    fans_out: bool,

    /// Local indices of this call's operations, in the order the assistant
    /// wrote them.
    ops: Vec<usize>,

    /// How many of `ops` have been handed to the execution loop.
    started: usize,

    /// One-based position of the first operation that failed, once one has.
    first_failure: Option<usize>,
}

/// Why an operation was never started.
enum NotRun {
    /// An earlier operation of the same call failed under `on_error = "stop"`.
    ///
    /// Carries that operation's one-based position, which the folded result
    /// names so the assistant can see which failure stopped the rest.
    AfterFailure { after: usize },

    /// The call was cancelled before this operation started.
    ///
    /// The caller writes the cancellation response into `results`, which the
    /// fold prefers, so this only exists to let the execution loop finish: an
    /// operation that never started will never send an event to wait for.
    Cancelled,
}

/// The execution loop's view of what to run and what to report.
pub(crate) struct Schedule {
    groups: Vec<Group>,

    /// Which group each local index belongs to.
    owner: Vec<usize>,

    /// Executors awaiting their turn, indexed by local index.
    ///
    /// Taken out when the operation starts; a `None` here means the operation
    /// either started already or was decided before it could.
    queued: Vec<Option<Box<dyn Executor>>>,

    /// Operations decided before the loop began, by local index.
    ///
    /// These hold the response the permission phase produced, which is folded
    /// in place rather than executed.
    predecided: Vec<Option<ToolCallResponse>>,

    /// Operations that will never start, by local index.
    not_run: Vec<Option<NotRun>>,
}

impl Schedule {
    /// Flatten the groups into local indices.
    pub(crate) fn new(groups: Vec<(usize, ExecutorGroup)>) -> Self {
        let mut schedule = Self {
            groups: Vec::with_capacity(groups.len()),
            owner: Vec::new(),
            queued: Vec::new(),
            predecided: Vec::new(),
            not_run: Vec::new(),
        };

        for (plan_index, group) in groups {
            let group_id = schedule.groups.len();
            let mut ops = Vec::with_capacity(group.ops.len());

            // A decision made before the loop began can already be a failure:
            // an argument formatter that errored resolves its operation to one.
            // Recorded here so a `stop` policy sees it on the very first
            // release, rather than only noticing failures that happen later.
            let mut first_failure = None;

            for (position, op) in group.ops.into_iter().enumerate() {
                let local = schedule.owner.len();
                schedule.owner.push(group_id);
                ops.push(local);

                match op {
                    GroupOp::Run(executor) => {
                        schedule.queued.push(Some(executor));
                        schedule.predecided.push(None);
                    }
                    GroupOp::Resolved(response) => {
                        if response.result.is_err() && first_failure.is_none() {
                            first_failure = Some(position + 1);
                        }
                        schedule.queued.push(None);
                        schedule.predecided.push(Some(response));
                    }
                }
                schedule.not_run.push(None);
            }

            schedule.groups.push(Group {
                plan_index,
                tool_id: group.tool_id,
                tool_name: group.tool_name,
                concurrency: group.fan_out.and_then(|f| f.concurrency),
                stop_on_error: group.fan_out.is_some_and(|f| f.stops_on_error()),
                fans_out: group.fan_out.is_some(),
                ops,
                started: 0,
                first_failure,
            });
        }

        schedule
    }

    /// Total operations across every call.
    pub(crate) fn total_ops(&self) -> usize {
        self.owner.len()
    }

    /// Take the operations that may start now.
    ///
    /// Called once before the loop begins and again each time an operation
    /// finishes, so a call with a concurrency limit releases its next operation
    /// as an earlier one completes.
    /// Operations a `stop` policy has ruled out are recorded here rather than
    /// returned, so the caller's "is everything accounted for?" check sees
    /// them.
    pub(crate) fn release(
        &mut self,
        results: &[Option<ToolCallResponse>],
    ) -> Vec<(usize, Box<dyn Executor>)> {
        let mut released = Vec::new();

        for group_id in 0..self.groups.len() {
            loop {
                let group = &self.groups[group_id];
                if group.started >= group.ops.len() {
                    break;
                }

                // A call that stops on error starts nothing further once one of
                // its operations has failed. Operations already running are left
                // alone: they are past the point where not starting them is an
                // option.
                if group.stop_on_error
                    && let Some(after) = group.first_failure
                {
                    for &local in &group.ops[group.started..] {
                        self.not_run[local] = Some(NotRun::AfterFailure { after });
                    }
                    let group = &mut self.groups[group_id];
                    group.started = group.ops.len();
                    break;
                }

                if let Some(limit) = group.concurrency {
                    let in_flight = group.ops[..group.started]
                        .iter()
                        .filter(|&&local| {
                            results[local].is_none()
                                && self.predecided[local].is_none()
                                && self.not_run[local].is_none()
                        })
                        .count();
                    if in_flight >= limit {
                        break;
                    }
                }

                let local = group.ops[group.started];
                self.groups[group_id].started += 1;

                // A predecided operation occupies its slot without running, so
                // it never counts against the concurrency limit and never
                // blocks the next one.
                if let Some(executor) = self.queued[local].take() {
                    released.push((local, executor));
                }
            }
        }

        released
    }

    /// Record that a local index finished, so a `stop` policy can act on it.
    ///
    /// Reads the failure off the response, which is what the assistant will
    /// receive.
    /// Use [`record_failure`] where the response has already been through
    /// result-mode policy, which can replace a failure with a success.
    ///
    /// [`record_failure`]: Self::record_failure
    pub(crate) fn record_outcome(&mut self, local: usize, response: &ToolCallResponse) {
        if response.result.is_ok() {
            return;
        }

        self.record_failure(local);
    }

    /// Record that a local index failed, whatever response the assistant ends
    /// up seeing for it.
    ///
    /// `result = "skip"` and a declined `result = "ask"` prompt both replace a
    /// failed response with a success before it reaches `results`, so a `stop`
    /// policy reading the response alone would release the next operation after
    /// a failure it was configured to stop on.
    pub(crate) fn record_failure(&mut self, local: usize) {
        let group_id = self.owner[local];
        let group = &mut self.groups[group_id];
        let position = group
            .ops
            .iter()
            .position(|&op| op == local)
            .map_or(1, |index| index + 1);

        if group.first_failure.is_none_or(|first| position < first) {
            group.first_failure = Some(position);
        }
    }

    /// Whether every operation is accounted for: finished, decided before it
    /// ran, or ruled out.
    pub(crate) fn all_accounted_for(&self, results: &[Option<ToolCallResponse>]) -> bool {
        (0..self.owner.len()).all(|local| self.is_accounted_for(local, results))
    }

    /// Whether one operation is accounted for.
    pub(crate) fn is_accounted_for(
        &self,
        local: usize,
        results: &[Option<ToolCallResponse>],
    ) -> bool {
        results[local].is_some()
            || self.predecided[local].is_some()
            || self.not_run[local].is_some()
    }

    /// Local indices of every operation that has not reported yet, whether it
    /// is running or still queued.
    ///
    /// Used when the user cancels, to decide which operations answer with the
    /// cancellation response.
    pub(crate) fn unfinished(&self, results: &[Option<ToolCallResponse>]) -> Vec<usize> {
        (0..self.owner.len())
            .filter(|&local| !self.is_accounted_for(local, results))
            .collect()
    }

    /// Give up on every operation that has not started yet.
    ///
    /// Called when the user cancels.
    /// Without this the execution loop would wait forever for operations that
    /// were never spawned and so will never send a result: an unbounded call
    /// has everything in flight, but a call with a concurrency limit is holding
    /// some back by design.
    pub(crate) fn abandon_unstarted(&mut self) {
        for group in &mut self.groups {
            for &local in &group.ops[group.started..] {
                self.not_run[local] = Some(NotRun::Cancelled);
            }
            group.started = group.ops.len();
        }
    }

    /// The tool a local index belongs to, for a message that needs to name it.
    pub(crate) fn tool_name(&self, local: usize) -> &str {
        &self.groups[self.owner[local]].tool_name
    }

    /// The tool call id a local index answers to.
    pub(crate) fn tool_id(&self, local: usize) -> &str {
        &self.groups[self.owner[local]].tool_id
    }

    /// Fold each call's operations into the one response it answers with.
    ///
    /// Operation order follows the assistant's, not completion order, so the
    /// sections line up with the `ops` array it wrote.
    pub(crate) fn fold(
        mut self,
        results: Vec<Option<ToolCallResponse>>,
    ) -> Vec<(usize, ToolCallResponse)> {
        let mut results: Vec<Option<ToolCallResponse>> = results;

        self.groups
            .drain(..)
            .map(|group| {
                let outcomes: Vec<OperationOutcome> = group
                    .ops
                    .iter()
                    .map(|&local| {
                        // `results` is consulted first so a cancellation written
                        // over an abandoned operation wins over its "not run"
                        // marker, which only existed to end the wait.
                        let response = results[local]
                            .take()
                            .or_else(|| self.predecided[local].take());

                        match (response, self.not_run[local].take()) {
                            (Some(response), _) => match response.result {
                                Ok(content) => OperationOutcome::Ok(content),
                                Err(message) => OperationOutcome::Error(message),
                            },
                            (None, Some(NotRun::AfterFailure { after })) => {
                                OperationOutcome::NotRun { after }
                            }
                            (None, Some(NotRun::Cancelled)) => {
                                OperationOutcome::Error("Operation cancelled.".to_owned())
                            }
                            (None, None) => {
                                OperationOutcome::Error("Tool did not complete".to_owned())
                            }
                        }
                    })
                    .collect();

                (group.plan_index, ToolCallResponse {
                    id: group.tool_id,
                    result: fan_out::fold_call(group.fans_out, outcomes),
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
