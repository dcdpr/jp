use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, error::TryRecvError};

use super::*;

/// Push a handler scope onto the router state.
fn push_handler(inner: &Arc<RouterInner>) -> (InterruptGuard, mpsc::Receiver<()>) {
    inner.push_handler(None)
}

/// A fixed conversation id, distinct per `secs`.
fn conversation(secs: u64) -> ConversationId {
    ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + Duration::from_secs(secs),
    )
    .unwrap()
}

/// A targeted interrupt reaches the named scope and nothing else.
///
/// The failure this guards against is stopping the wrong turn: with several
/// running, the topmost handler is very likely not the one that was asked for.
#[test]
fn a_scoped_interrupt_notifies_only_its_own_scope() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let wanted = conversation(1_700_000_000);
    let other = conversation(1_700_000_001);

    let (_guard_wanted, mut rx_wanted) = inner.push_handler(Some(wanted));
    // Pushed after, so it is topmost and would be the one a Ctrl-C reached.
    let (_guard_other, mut rx_other) = inner.push_handler(Some(other));
    let (_guard_plain, mut rx_plain) = push_handler(&inner);

    assert!(inner.notify_scope(wanted));

    assert_eq!(rx_wanted.try_recv(), Ok(()));
    assert_eq!(rx_other.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(rx_plain.try_recv(), Err(TryRecvError::Empty));
}

/// A scope with no handler is not an error: its work already finished, so there
/// was nothing left to interrupt.
#[test]
fn interrupting_an_unknown_scope_reports_that_nothing_was_reached() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);

    assert!(!inner.notify_scope(conversation(1_700_000_000)));

    assert_eq!(
        rx.try_recv(),
        Err(TryRecvError::Empty),
        "an unscoped handler is not a fallback target"
    );
}

/// A dropped guard takes its scope with it, so a later interrupt finds nothing
/// rather than a stale channel.
#[test]
fn a_finished_scope_is_no_longer_reachable() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let id = conversation(1_700_000_000);

    let (guard, _rx) = inner.push_handler(Some(id));
    assert!(inner.notify_scope(id));

    drop(guard);
    assert!(!inner.notify_scope(id));
}

/// A Ctrl-C still goes to whichever handler is topmost, scoped or not.
///
/// The scope is extra information for targeted interrupts, not a change to how
/// the keyboard path chooses.
#[test]
fn a_scope_does_not_change_where_a_keypress_lands() {
    let inner = RouterInner::new(Duration::from_secs(2));

    let (_guard_bottom, mut rx_bottom) = inner.push_handler(Some(conversation(1_700_000_000)));
    let (_guard_top, mut rx_top) = inner.push_handler(Some(conversation(1_700_000_001)));

    assert_eq!(inner.route_interrupt(Instant::now()), Routed::Handler);

    assert_eq!(rx_top.try_recv(), Ok(()));
    assert_eq!(rx_bottom.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn interrupt_without_handlers_requests_shutdown() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Shutdown);
    assert!(inner.shutdown_token.is_cancelled());
}

#[test]
fn interrupt_notifies_topmost_handler_only() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard_bottom, mut rx_bottom) = push_handler(&inner);
    let (_guard_top, mut rx_top) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    assert_eq!(rx_top.try_recv(), Ok(()));
    assert_eq!(rx_bottom.try_recv(), Err(TryRecvError::Empty));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn second_interrupt_within_cooldown_bypasses_handlers() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    assert_eq!(rx.try_recv(), Ok(()));

    let second = now + Duration::from_millis(500);
    assert_eq!(
        inner.route_at(OsSignal::Interrupt, second),
        Routed::Shutdown
    );
    assert!(inner.shutdown_token.is_cancelled());
    // The handler was bypassed: no second notification.
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn third_interrupt_within_cooldown_exits() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, _rx) = push_handler(&inner);
    let now = Instant::now();

    inner.route_at(OsSignal::Interrupt, now);
    inner.route_at(OsSignal::Interrupt, now + Duration::from_millis(500));

    assert_eq!(
        inner.route_at(OsSignal::Interrupt, now + Duration::from_secs(1)),
        Routed::Exit(130),
    );
}

#[test]
fn interrupt_after_shutdown_began_exits() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let now = Instant::now();

    // Graceful shutdown began through another path (e.g. SIGTERM).
    inner.shutdown_token.cancel();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Exit(130),);
}

#[test]
fn cooldown_resets_escalation_counter() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    assert_eq!(rx.try_recv(), Ok(()));

    // Past the cooldown, the next press counts as a fresh first press.
    let later = now + Duration::from_secs(3);
    assert_eq!(inner.route_at(OsSignal::Interrupt, later), Routed::Handler);
    assert_eq!(rx.try_recv(), Ok(()));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn full_notification_channel_counts_as_notified() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);

    // The handler hasn't consumed the pending notification; a fresh first
    // press (past the cooldown) is a no-op send, not an error.
    let later = now + Duration::from_secs(3);
    assert_eq!(inner.route_at(OsSignal::Interrupt, later), Routed::Handler);
    assert_eq!(rx.try_recv(), Ok(()));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn closed_notification_channel_falls_back_to_shutdown() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, rx) = push_handler(&inner);
    let now = Instant::now();

    // The handler's event loop exited, but the guard hasn't dropped yet.
    drop(rx);

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Shutdown);
    assert!(inner.shutdown_token.is_cancelled());
}

#[test]
fn dropping_guard_deregisters_handler() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard_bottom, mut rx_bottom) = push_handler(&inner);
    let (guard_top, mut rx_top) = push_handler(&inner);
    let now = Instant::now();

    drop(guard_top);

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    assert_eq!(rx_bottom.try_recv(), Ok(()));
    // Deregistration dropped the stored sender without ever notifying.
    assert_eq!(rx_top.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn guards_can_drop_out_of_order() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (guard_bottom, mut rx_bottom) = push_handler(&inner);
    let (_guard_top, mut rx_top) = push_handler(&inner);
    let now = Instant::now();

    // The outer scope unwinds before the inner one; the topmost handler must
    // remain intact.
    drop(guard_bottom);

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    assert_eq!(rx_top.try_recv(), Ok(()));
    // Deregistration dropped the stored sender without ever notifying.
    assert_eq!(rx_bottom.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn terminate_requests_shutdown() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Terminate, now), Routed::Shutdown);
    assert!(inner.shutdown_token.is_cancelled());
}

#[test]
fn terminate_bypasses_handler_stack() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Terminate, now), Routed::Shutdown);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(inner.shutdown_token.is_cancelled());
}

#[test]
fn quit_exits() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Quit, now), Routed::Exit(131));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn decline_notifies_next_handler_down() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard_bottom, mut rx_bottom) = push_handler(&inner);
    let (_guard_top, mut rx_top) = push_handler(&inner);

    inner.notify_next_or_shutdown();

    assert_eq!(rx_bottom.try_recv(), Ok(()));
    assert_eq!(rx_top.try_recv(), Err(TryRecvError::Empty));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn decline_with_single_handler_requests_shutdown() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);

    inner.notify_next_or_shutdown();

    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(inner.shutdown_token.is_cancelled());
}

#[test]
fn escalation_counter_bumps_and_resets() {
    let mut state = EscalationState::new(Duration::from_secs(2));
    let now = Instant::now();

    assert_eq!(state.bump(now), 1);
    assert_eq!(state.bump(now + Duration::from_secs(1)), 2);
    assert_eq!(state.bump(now + Duration::from_secs(2)), 3);

    // A press past the cooldown starts over.
    assert_eq!(state.bump(now + Duration::from_secs(10)), 1);
}

/// A clone is a second handle onto one router, not a second router.
///
/// Work running away from the thread that started it holds a clone, and a
/// handler it registers has to be one the signal task reaches.
/// Were the state duplicated instead of shared, the press would find an empty
/// stack and fall through to requesting shutdown, which is what this pins.
#[tokio::test]
async fn a_cloned_router_shares_the_handler_stack() {
    let (router, signals) = super::testing::test_router();
    let clone = router.clone();

    // Registered through the clone, delivered through the original's source.
    let (_guard, mut interrupt_rx) = clone.push_handler();

    signals.interrupt().await;

    // Bounded, because the failure this guards against is a notification that
    // never arrives: the guard keeps the sender alive, so an unshared stack would
    // leave `recv` blocked rather than closed, and the test would hang instead of
    // failing.
    tokio::time::timeout(Duration::from_secs(5), interrupt_rx.recv())
        .await
        .expect("a handler pushed on a clone is reached by the signal task")
        .expect("the notification channel stayed open");

    // The press stopped at the handler, so neither of these ran.
    assert!(!router.shutdown_token().is_cancelled());
    assert!(!clone.shutdown_token().is_cancelled());
    assert!(signals.exit_codes().is_empty());
}

/// Drives the full Ctrl-C escalation ladder through the real signal task —
/// handler notification, shutdown, process exit — without ending the test
/// process: the injected exit action records the code instead of exiting.
#[tokio::test]
async fn escalation_ladder_reaches_exit_through_signal_task() {
    let (router, signals) = super::testing::test_router();
    let (_guard, mut interrupt_rx) = router.push_handler();

    // First press: notifies the topmost handler; no shutdown, no exit.
    signals.interrupt().await;
    interrupt_rx
        .recv()
        .await
        .expect("handler should be notified");
    assert!(!router.shutdown_token().is_cancelled());
    assert!(signals.exit_codes().is_empty());

    // Second press within the cooldown: bypasses the handler and requests a
    // graceful shutdown.
    signals.interrupt().await;
    router.shutdown_token().cancelled().await;
    assert!(signals.exit_codes().is_empty());

    // Third press: the exit action fires with the SIGINT exit code. Delivery
    // is asynchronous, so poll for the recording rather than asserting
    // immediately.
    signals.interrupt().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while signals.exit_codes().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the third press should reach the exit action");
    assert_eq!(signals.exit_codes(), vec![130]);
}
