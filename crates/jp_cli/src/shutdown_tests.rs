use super::*;

async fn command_taking(after: Duration) -> cmd::Output {
    tokio::time::sleep(after).await;
    Ok(())
}

async fn command_claiming(
    budgets: ShutdownBudgets,
    unclaimed: Duration,
    budget: Duration,
    held: Duration,
) -> cmd::Output {
    tokio::time::sleep(unclaimed).await;
    let claim = budgets.claim(budget);
    tokio::time::sleep(held).await;
    drop(claim);
    tokio::time::sleep(Duration::from_mins(1)).await;
    Ok(())
}

fn shutdown_requested_in(after: Duration) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        cancel.cancel();
    });

    token
}

#[test]
fn a_budget_is_claimed_until_its_guard_is_dropped() {
    let budgets = ShutdownBudgets::default();
    assert_eq!(budgets.longest(), None);

    let claim = budgets.claim(Duration::from_secs(5));
    assert_eq!(budgets.longest(), Some(Duration::from_secs(5)));

    drop(claim);
    assert_eq!(budgets.longest(), None);
}

#[test]
fn the_longest_budget_still_claimed_is_the_budget() {
    let budgets = ShutdownBudgets::default();

    let short = budgets.claim(Duration::from_secs(5));
    let long = budgets.claim(Duration::from_secs(9));
    assert_eq!(budgets.longest(), Some(Duration::from_secs(9)));

    // Released in the order they were claimed, which a stack would get wrong.
    drop(short);
    assert_eq!(budgets.longest(), Some(Duration::from_secs(9)));

    drop(long);
    assert_eq!(budgets.longest(), None);
}

#[test]
fn two_claims_of_the_same_budget_are_released_one_at_a_time() {
    let budgets = ShutdownBudgets::default();

    let first = budgets.claim(Duration::from_secs(5));
    let second = budgets.claim(Duration::from_secs(5));

    drop(first);
    assert_eq!(budgets.longest(), Some(Duration::from_secs(5)));

    drop(second);
    assert_eq!(budgets.longest(), None);
}

#[test]
fn a_clone_sees_the_same_claims() {
    let budgets = ShutdownBudgets::default();
    let clone = budgets.clone();

    let claim = budgets.claim(Duration::from_secs(5));
    assert_eq!(clone.longest(), Some(Duration::from_secs(5)));

    drop(claim);
    assert_eq!(clone.longest(), None);
}

#[tokio::test]
async fn a_shutdown_request_runs_what_was_registered_for_it() {
    let shutdown = CancellationToken::new();
    let (ran, has_run) = tokio::sync::oneshot::channel();

    on_shutdown(&Handle::current(), shutdown.clone(), move || {
        ran.send(()).ok();
    });

    // Nothing runs until the request.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut has_run = has_run;
    assert!(has_run.try_recv().is_err());

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), has_run)
        .await
        .expect("it ran once shutdown was requested")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_request_drops_a_command_with_nothing_claimed_at_once() {
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(
        command_taking(Duration::from_mins(1)),
        &shutdown,
        &ShutdownBudgets::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(error.code.get(), 130);
    assert_eq!(started.elapsed(), Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn a_command_holding_a_claim_keeps_running_after_a_shutdown_request() {
    let budgets = ShutdownBudgets::default();
    let _claim = budgets.claim(Duration::from_secs(5));
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    until_shutdown(command_taking(Duration::from_secs(3)), &shutdown, &budgets)
        .await
        .unwrap();

    assert_eq!(started.elapsed(), Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn a_command_still_running_when_its_budget_runs_out_is_dropped() {
    let budgets = ShutdownBudgets::default();
    let _claim = budgets.claim(Duration::from_secs(5));
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(command_taking(Duration::from_mins(1)), &shutdown, &budgets)
        .await
        .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(error.code.get(), 130);
    assert_eq!(started.elapsed(), Duration::from_secs(6));
}

#[tokio::test(start_paused = true)]
async fn a_budget_claimed_while_the_command_runs_is_the_one_a_later_request_gets() {
    let budgets = ShutdownBudgets::default();
    let shutdown = shutdown_requested_in(Duration::from_secs(3));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_mins(1),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(8));
}

#[tokio::test(start_paused = true)]
async fn a_request_before_a_budget_is_claimed_drops_the_command_at_once() {
    let budgets = ShutdownBudgets::default();
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_mins(1),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(budgets.longest(), None);
}

#[tokio::test(start_paused = true)]
async fn a_claim_released_before_the_request_leaves_the_command_to_be_dropped_at_once() {
    let budgets = ShutdownBudgets::default();
    let shutdown = shutdown_requested_in(Duration::from_secs(4));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::ZERO,
            Duration::from_secs(30),
            Duration::from_secs(2),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn a_claim_released_after_the_request_drops_the_command_then() {
    let budgets = ShutdownBudgets::default();
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::ZERO,
            Duration::from_secs(30),
            Duration::from_secs(3),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn a_longer_claim_made_after_the_request_extends_the_budget() {
    let budgets = ShutdownBudgets::default();
    let _short = budgets.claim(Duration::from_secs(5));
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::from_secs(2),
            Duration::from_secs(20),
            Duration::from_mins(1),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(21));
}

#[tokio::test(start_paused = true)]
async fn releasing_the_longest_claim_leaves_the_shorter_one_to_run_out() {
    let budgets = ShutdownBudgets::default();
    let _short = budgets.claim(Duration::from_secs(5));
    let shutdown = shutdown_requested_in(Duration::from_secs(1));
    let started = Instant::now();

    let error = until_shutdown(
        command_claiming(
            budgets.clone(),
            Duration::ZERO,
            Duration::from_secs(20),
            Duration::from_secs(3),
        ),
        &shutdown,
        &budgets,
    )
    .await
    .unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(started.elapsed(), Duration::from_secs(6));
}
