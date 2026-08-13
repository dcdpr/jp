use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, error::TryRecvError};

use super::*;

/// Push a handler scope onto the router state.
fn push_handler(inner: &Arc<RouterInner>) -> (InterruptGuard, mpsc::Receiver<()>) {
    inner.push_handler()
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
