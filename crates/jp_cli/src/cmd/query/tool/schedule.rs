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
use jp_tool::fan_out::{self, OperationOutcome};

use super::{
    coordinator::{ExecutorGroup, GroupOp},
    executor::{Executor, Review},
};

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

/// What folding a phase's operations back into their calls produces.
pub(crate) struct Folded {
    /// The review each call is recorded with, paired with its plan index.
    pub recorded: Vec<(usize, Review)>,

    /// Per fanned-out call, by tool call id, the review each operation's MCP
    /// call is acknowledged with.
    pub operations: Vec<(String, Vec<Review>)>,
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
        results: &[Option<Review>],
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
    pub(crate) fn all_accounted_for(&self, results: &[Option<Review>]) -> bool {
        (0..self.owner.len()).all(|local| self.is_accounted_for(local, results))
    }

    /// Whether one operation is accounted for.
    pub(crate) fn is_accounted_for(&self, local: usize, results: &[Option<Review>]) -> bool {
        results[local].is_some()
            || self.predecided[local].is_some()
            || self.not_run[local].is_some()
    }

    /// Local indices of every operation that has not reported yet, whether it
    /// is running or still queued.
    ///
    /// Used when the user cancels, to decide which operations answer with the
    /// cancellation response.
    pub(crate) fn unfinished(&self, results: &[Option<Review>]) -> Vec<usize> {
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

    /// Operations released to nobody yet, still waiting behind a concurrency
    /// limit.
    ///
    /// Each was prepared and approved before the loop began, so its MCP call is
    /// already open.
    pub(crate) fn queued(&self) -> impl Iterator<Item = &dyn Executor> {
        self.queued.iter().flatten().map(AsRef::as_ref)
    }

    /// The tool a local index belongs to, for a message that needs to name it.
    pub(crate) fn tool_name(&self, local: usize) -> &str {
        &self.groups[self.owner[local]].tool_name
    }

    /// The tool call id a local index answers to.
    pub(crate) fn tool_id(&self, local: usize) -> &str {
        &self.groups[self.owner[local]].tool_id
    }

    /// Fold each call's operations into the one review it is recorded with.
    ///
    /// Operation order follows the assistant's, not completion order, so the
    /// sections line up with the `ops` array it wrote.
    ///
    /// Also returns, per fanned-out call, the review each operation's own MCP
    /// call settled on, which is what acknowledging that call needs: the folded
    /// response is not any one operation's result.
    /// A call that does not fan out has no such entry, because its one
    /// operation's review is the review it is recorded with.
    pub(crate) fn fold(mut self, mut results: Vec<Option<Review>>) -> Folded {
        let mut recorded = Vec::with_capacity(self.groups.len());
        let mut operations = Vec::new();

        for group in self.groups.drain(..) {
            let mut outcomes = Vec::with_capacity(group.ops.len());
            let mut reviews = Vec::with_capacity(group.ops.len());

            for (position, &local) in group.ops.iter().enumerate() {
                // `results` is consulted first so a cancellation written over
                // an abandoned operation wins over its "not run" marker, which
                // only existed to end the wait.
                let review = results[local]
                    .take()
                    .or_else(|| self.predecided[local].take().map(Review::unchanged));

                let (outcome, review) = match (review, self.not_run[local].take()) {
                    (Some(review), _) => {
                        let outcome = match &review.response.result {
                            Ok(content) => OperationOutcome::Ok(content.clone()),
                            Err(message) => OperationOutcome::Error(message.clone()),
                        };
                        (outcome, review)
                    }
                    (None, Some(NotRun::AfterFailure { after })) => (
                        OperationOutcome::NotRun { after },
                        not_run(
                            &group.tool_id,
                            format!("Operation not run: stopped after operation {after} failed."),
                        ),
                    ),
                    (None, Some(NotRun::Cancelled)) => {
                        let message = "Operation cancelled.".to_owned();
                        (
                            OperationOutcome::Error(message.clone()),
                            not_run(&group.tool_id, message),
                        )
                    }
                    (None, None) => {
                        let message = "Tool did not complete".to_owned();
                        (
                            OperationOutcome::Error(message.clone()),
                            not_run(&group.tool_id, message),
                        )
                    }
                };

                outcomes.push(outcome);
                reviews.push(review.for_op(group.fans_out.then_some(position)));
            }

            // A call that does not fan out answers with its one operation's
            // review verbatim, including whether it failed and whether the Host
            // edited it. Folding would flatten a failure into a success
            // carrying error text.
            if !group.fans_out
                && let Some(review) = reviews.pop()
            {
                recorded.push((group.plan_index, review));
                continue;
            }

            // The folded body is JP's own construction, not anything one
            // operation offered.
            recorded.push((
                group.plan_index,
                Review::replaced(ToolCallResponse {
                    id: group.tool_id.clone(),
                    result: Ok(fan_out::fold(&outcomes)),
                }),
            ));
            operations.push((group.tool_id, reviews));
        }

        Folded {
            recorded,
            operations,
        }
    }
}

/// The review an operation that never ran settles its MCP call with.
fn not_run(tool_id: &str, message: String) -> Review {
    Review::replaced(ToolCallResponse {
        id: tool_id.to_owned(),
        result: Err(message),
    })
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
