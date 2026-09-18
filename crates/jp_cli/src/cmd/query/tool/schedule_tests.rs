use jp_config::conversation::tool::FanOutOnError;
use jp_llm::tool::executor::MockExecutor;

use super::*;

/// A group of `count` runnable operations under one tool call id.
fn group(
    tool_id: &str,
    count: usize,
    concurrency: Option<usize>,
    on_error: FanOutOnError,
) -> ExecutorGroup {
    ExecutorGroup {
        tool_id: tool_id.to_owned(),
        tool_name: "my_tool".to_owned(),
        fan_out: Some(jp_config::conversation::tool::FanOut {
            concurrency,
            on_error,
        }),
        ops: (0..count)
            .map(|_| {
                GroupOp::Run(Box::new(MockExecutor::completed(tool_id, "my_tool", "ok"))
                    as Box<dyn Executor>)
            })
            .collect(),
    }
}

/// A call to a tool without fan-out: exactly one operation, no envelope.
fn single(tool_id: &str) -> ExecutorGroup {
    ExecutorGroup {
        tool_id: tool_id.to_owned(),
        tool_name: "my_tool".to_owned(),
        fan_out: None,
        ops: vec![GroupOp::Run(
            Box::new(MockExecutor::completed(tool_id, "my_tool", "ok")) as Box<dyn Executor>,
        )],
    }
}

fn ok(tool_id: &str, content: &str) -> ToolCallResponse {
    ToolCallResponse {
        id: tool_id.to_owned(),
        result: Ok(content.to_owned()),
    }
}

fn err(tool_id: &str, message: &str) -> ToolCallResponse {
    ToolCallResponse {
        id: tool_id.to_owned(),
        result: Err(message.to_owned()),
    }
}

#[test]
fn an_unbounded_call_releases_every_operation_at_once() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 3, None, FanOutOnError::Continue))]);
    let results = vec![None; schedule.total_ops()];

    let released = schedule.release(&results);

    assert_eq!(released.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![
        0, 1, 2
    ]);
}

#[test]
fn a_sequential_call_releases_one_operation_at_a_time() {
    let mut schedule = Schedule::new(vec![(
        0,
        group("call_1", 3, Some(1), FanOutOnError::Continue),
    )]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![0],
        "only the first operation starts"
    );

    assert!(
        schedule.release(&results).is_empty(),
        "nothing else starts while the first is still running"
    );

    results[0] = Some(ok("call_1", "first"));
    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![1],
        "finishing the first releases the second"
    );
}

#[test]
fn a_bounded_call_keeps_the_configured_number_in_flight() {
    let mut schedule = Schedule::new(vec![(
        0,
        group("call_1", 5, Some(2), FanOutOnError::Continue),
    )]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    results[1] = Some(ok("call_1", "second"));
    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![2],
        "one slot freed releases exactly one operation"
    );
}

/// Operations of different calls do not compete for each other's slots.
#[test]
fn concurrency_is_counted_per_call() {
    let mut schedule = Schedule::new(vec![
        (0, group("call_1", 2, Some(1), FanOutOnError::Continue)),
        (1, group("call_2", 2, Some(1), FanOutOnError::Continue)),
    ]);
    let results = vec![None; schedule.total_ops()];

    let released = schedule.release(&results);

    assert_eq!(
        released.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![0, 2],
        "each call starts its own first operation"
    );
}

#[test]
fn stop_on_error_starts_nothing_after_a_failure() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 4, Some(1), FanOutOnError::Stop))]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);
    results[0] = Some(err("call_1", "boom"));
    schedule.record_outcome(0, results[0].as_ref().expect("recorded"));

    assert!(
        schedule.release(&results).is_empty(),
        "the remaining operations never start"
    );
    assert!(
        schedule.all_accounted_for(&results),
        "operations that will never start are accounted for, so the loop can exit"
    );

    let folded = schedule.fold(results);
    assert_eq!(folded.len(), 1);
    assert_eq!(
        folded[0].1.result,
        Ok(
            "[1/4] error\nboom\n\n[2/4] not run (stopped after operation 1 failed)\n\n[3/4] not \
             run (stopped after operation 1 failed)\n\n[4/4] not run (stopped after operation 1 \
             failed)\n"
                .to_owned()
        )
    );
}

#[test]
fn continue_on_error_runs_every_operation() {
    let mut schedule = Schedule::new(vec![(
        0,
        group("call_1", 3, Some(1), FanOutOnError::Continue),
    )]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);
    results[0] = Some(err("call_1", "boom"));
    schedule.record_outcome(0, results[0].as_ref().expect("recorded"));

    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![1],
        "a failure does not stop the rest"
    );
}

/// The envelope is invisible at N=1: a one-operation fan-out call answers with
/// its operation's output and nothing else.
#[test]
fn a_single_operation_folds_without_framing() {
    let schedule = Schedule::new(vec![(0, group("call_1", 1, None, FanOutOnError::Continue))]);
    let results = vec![Some(ok("call_1", "file contents"))];

    let folded = schedule.fold(results);

    assert_eq!(folded[0].1.result, Ok("file contents".to_owned()));
}

/// A call to a tool without fan-out keeps its failure a failure.
/// Folding it would turn an error into a success carrying error text, which the
/// assistant reads as the tool having worked.
#[test]
fn a_call_without_fan_out_passes_its_error_through() {
    let schedule = Schedule::new(vec![(0, single("call_1"))]);
    let results = vec![Some(err("call_1", "not found"))];

    let folded = schedule.fold(results);

    assert_eq!(folded[0].1.result, Err("not found".to_owned()));
}

#[test]
fn folding_preserves_plan_indices_and_tool_ids() {
    let schedule = Schedule::new(vec![
        (3, group("call_a", 2, None, FanOutOnError::Continue)),
        (7, single("call_b")),
    ]);
    let results = vec![
        Some(ok("call_a", "one")),
        Some(ok("call_a", "two")),
        Some(ok("call_b", "three")),
    ];

    let folded = schedule.fold(results);

    assert_eq!(folded[0].0, 3);
    assert_eq!(folded[0].1.id, "call_a");
    assert_eq!(
        folded[0].1.result,
        Ok("[1/2] ok\none\n\n[2/2] ok\ntwo\n".to_owned())
    );
    assert_eq!(folded[1].0, 7);
    assert_eq!(folded[1].1.id, "call_b");
    assert_eq!(folded[1].1.result, Ok("three".to_owned()));
}

/// Operations are folded in the order the assistant wrote them, not the order
/// they happened to finish in.
#[test]
fn folding_follows_the_assistants_order_not_completion_order() {
    let schedule = Schedule::new(vec![(0, group("call_1", 3, None, FanOutOnError::Continue))]);
    let results = vec![
        Some(ok("call_1", "first")),
        Some(ok("call_1", "second")),
        Some(ok("call_1", "third")),
    ];

    let folded = schedule.fold(results);

    assert_eq!(
        folded[0].1.result,
        Ok("[1/3] ok\nfirst\n\n[2/3] ok\nsecond\n\n[3/3] ok\nthird\n".to_owned())
    );
}

#[test]
fn a_predecided_operation_never_runs_but_still_reports() {
    let mut group = group("call_1", 2, None, FanOutOnError::Continue);
    group.ops[0] = GroupOp::Resolved(ok("call_1", "Tool skipped by user."));

    let mut schedule = Schedule::new(vec![(0, group)]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    let released = schedule.release(&results);
    assert_eq!(
        released.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![1],
        "the skipped operation is not handed to the execution loop"
    );

    results[1] = Some(ok("call_1", "ran"));
    assert!(schedule.all_accounted_for(&results));

    let folded = schedule.fold(results);
    assert_eq!(
        folded[0].1.result,
        Ok("[1/2] ok\nTool skipped by user.\n\n[2/2] ok\nran\n".to_owned())
    );
}

/// A skipped operation holds a slot without occupying one, so a sequential call
/// does not stall waiting for something that will never report.
#[test]
fn a_predecided_operation_does_not_hold_a_concurrency_slot() {
    let mut group = group("call_1", 3, Some(1), FanOutOnError::Continue);
    group.ops[0] = GroupOp::Resolved(ok("call_1", "skipped"));

    let mut schedule = Schedule::new(vec![(0, group)]);
    let results = vec![None; schedule.total_ops()];

    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![1],
        "the skipped operation passes through and the next one starts"
    );
}

#[test]
fn unfinished_lists_the_operations_that_have_not_reported() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 3, None, FanOutOnError::Continue))]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];
    schedule.release(&results);

    results[1] = Some(ok("call_1", "done"));

    assert_eq!(schedule.unfinished(&results), vec![0, 2]);
}

/// A cancelled call holding operations behind a concurrency limit must still
/// let the execution loop finish.
/// Those operations were never spawned, so no result will ever arrive for them
/// and the loop would wait forever.
#[test]
fn abandoning_unstarted_operations_lets_a_cancelled_call_finish() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 4, Some(1), FanOutOnError::Stop))]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);
    assert!(!schedule.all_accounted_for(&results));

    // The user cancels: every operation that has not reported answers with the
    // cancellation response, including the three that never started.
    let cancelled = schedule.unfinished(&results);
    assert_eq!(cancelled, vec![0, 1, 2, 3]);

    schedule.abandon_unstarted();

    // The one running operation reports, and the loop can now see that nothing
    // else is outstanding.
    results[0] = Some(ok("call_1", "cancelled"));
    assert!(
        schedule.all_accounted_for(&results),
        "operations that were never spawned must not keep the loop waiting"
    );

    for &index in &cancelled {
        results[index] = Some(ok("call_1", "cancelled"));
    }

    let folded = schedule.fold(results);
    assert_eq!(
        folded[0].1.result,
        Ok(
            "[1/4] ok\ncancelled\n\n[2/4] ok\ncancelled\n\n[3/4] ok\ncancelled\n\n[4/4] \
             ok\ncancelled\n"
                .to_owned()
        ),
        "a written cancellation wins over the marker that ended the wait"
    );
}

/// An operation that failed before the loop began (an argument formatter that
/// errored) stops the rest under `on_error = "stop"`, the same as one that
/// failed while running.
#[test]
fn a_predecided_failure_stops_the_operations_behind_it() {
    let mut group = group("call_1", 3, Some(1), FanOutOnError::Stop);
    group.ops[0] = GroupOp::Resolved(err("call_1", "formatter failed"));

    let mut schedule = Schedule::new(vec![(0, group)]);
    let results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    assert!(
        schedule.release(&results).is_empty(),
        "nothing starts behind a failure that was already decided"
    );
    assert!(schedule.all_accounted_for(&results));

    let folded = schedule.fold(results);
    assert_eq!(
        folded[0].1.result,
        Ok(
            "[1/3] error\nformatter failed\n\n[2/3] not run (stopped after operation 1 \
             failed)\n\n[3/3] not run (stopped after operation 1 failed)\n"
                .to_owned()
        )
    );
}

/// `result = "skip"` answers the assistant with a success even when the tool
/// failed, so a `stop` policy reading the response alone would release the next
/// operation after a failure it was configured to stop on.
#[test]
fn a_failure_hidden_by_result_policy_still_stops_the_rest() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 3, Some(1), FanOutOnError::Stop))]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);

    // What `result = "skip"` leaves behind: the tool failed, the assistant is
    // told otherwise.
    results[0] = Some(ok("call_1", "Result delivery skipped by configuration."));
    schedule.record_failure(0);

    assert!(
        schedule.release(&results).is_empty(),
        "the stop policy acts on what the tool did, not on what the assistant was told"
    );
    assert!(schedule.all_accounted_for(&results));
}

/// The response-reading path is the backstop, and must not invent a failure
/// where the tool reported none.
#[test]
fn a_successful_operation_does_not_stop_the_rest() {
    let mut schedule = Schedule::new(vec![(0, group("call_1", 3, Some(1), FanOutOnError::Stop))]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);
    results[0] = Some(ok("call_1", "wrote"));
    schedule.record_outcome(0, results[0].as_ref().expect("recorded"));

    assert_eq!(
        schedule
            .release(&results)
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![1],
        "a success releases the next operation"
    );
}

/// Abandoning is what every interrupt outcome that cancels the token reaches
/// for: a restart re-runs the batch from the top and an escalation is a
/// shutdown, so neither wants the schedule handing out more work on the way
/// out.
#[test]
fn an_abandoned_schedule_releases_nothing_further() {
    let mut schedule = Schedule::new(vec![(
        0,
        group("call_1", 4, Some(1), FanOutOnError::Continue),
    )]);
    let mut results: Vec<Option<ToolCallResponse>> = vec![None; schedule.total_ops()];

    schedule.release(&results);
    schedule.abandon_unstarted();

    // The one running operation finishes, which would ordinarily free its slot.
    results[0] = Some(ok("call_1", "wrote"));

    assert!(
        schedule.release(&results).is_empty(),
        "a finished operation must not free a slot for one that was abandoned"
    );
    assert!(schedule.all_accounted_for(&results));
}

#[test]
fn schedule_reports_the_tool_behind_a_local_index() {
    let schedule = Schedule::new(vec![
        (0, group("call_a", 2, None, FanOutOnError::Continue)),
        (1, single("call_b")),
    ]);

    assert_eq!(schedule.tool_id(0), "call_a");
    assert_eq!(schedule.tool_id(1), "call_a");
    assert_eq!(schedule.tool_id(2), "call_b");
    assert_eq!(schedule.tool_name(2), "my_tool");
}
