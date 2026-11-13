use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{fs, io::Write as _, process::Command};

#[cfg(unix)]
use camino_tempfile::tempdir;
#[cfg(unix)]
use jp_inquire::{InlineOption, InlineSelect};
#[cfg(unix)]
use jp_pty::{CommandBuilder, Size, Terminal};
use tokio::sync::mpsc::{self, error::TryRecvError};

use super::*;

/// Environment variable naming the PID file for
/// [`a_prompt_left_open_until_the_process_exits`].
#[cfg(unix)]
const EXIT_PROBE_PID_FILE: &str = "JP_TEST_EXIT_PROBE_PID_FILE";

#[cfg(unix)]
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Push a handler scope onto the router state.
fn push_handler(inner: &Arc<RouterInner>) -> (InterruptGuard, mpsc::Receiver<InterruptNotice>) {
    inner.push_handler()
}

/// Take a delivered press off the channel and drop it unresolved.
///
/// Dropping models a handler that was notified but neither acted on the press
/// nor declined it, which is what keeps it on the escalation ladder.
fn took_notice(rx: &mut mpsc::Receiver<InterruptNotice>) -> bool {
    rx.try_recv().is_ok()
}

/// Why no press was waiting on the channel.
fn recv_error(rx: &mut mpsc::Receiver<InterruptNotice>) -> Option<TryRecvError> {
    rx.try_recv().err()
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
    assert!(took_notice(&mut rx_top));
    assert_eq!(recv_error(&mut rx_bottom), Some(TryRecvError::Empty));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn second_interrupt_within_cooldown_bypasses_handlers() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    // Taken and dropped unresolved: the handler did nothing with the press.
    assert!(took_notice(&mut rx));

    let second = now + Duration::from_millis(500);
    assert_eq!(
        inner.route_at(OsSignal::Interrupt, second),
        Routed::Shutdown
    );
    assert!(inner.shutdown_token.is_cancelled());
    // The handler was bypassed: no second notification.
    assert_eq!(recv_error(&mut rx), Some(TryRecvError::Empty));
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
    assert!(took_notice(&mut rx));

    // Past the cooldown, the next press counts as a fresh first press.
    let later = now + Duration::from_secs(3);
    assert_eq!(inner.route_at(OsSignal::Interrupt, later), Routed::Handler);
    assert!(took_notice(&mut rx));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn answered_interrupt_resets_escalation_counter() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (_guard, mut rx) = push_handler(&inner);
    let now = Instant::now();

    // The handler showed its menu and the user answered it.
    assert_eq!(inner.route_at(OsSignal::Interrupt, now), Routed::Handler);
    rx.try_recv()
        .expect("the handler should be notified")
        .handled();

    // Well inside the cooldown, but the previous press was answered, so this
    // one opens the menu again rather than escalating past it.
    let second = now + Duration::from_millis(1400);
    assert_eq!(inner.route_at(OsSignal::Interrupt, second), Routed::Handler);
    assert!(took_notice(&mut rx));
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
    assert!(took_notice(&mut rx));
    assert_eq!(recv_error(&mut rx), Some(TryRecvError::Empty));
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
    assert!(took_notice(&mut rx_bottom));
    // Deregistration dropped the stored sender without ever notifying.
    assert_eq!(recv_error(&mut rx_top), Some(TryRecvError::Disconnected));
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
    assert!(took_notice(&mut rx_top));
    // Deregistration dropped the stored sender without ever notifying.
    assert_eq!(recv_error(&mut rx_bottom), Some(TryRecvError::Disconnected));
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
    assert_eq!(recv_error(&mut rx), Some(TryRecvError::Empty));
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
    let (guard_top, mut rx_top) = push_handler(&inner);

    inner.notify_below(guard_top.id);

    assert!(took_notice(&mut rx_bottom));
    assert_eq!(recv_error(&mut rx_top), Some(TryRecvError::Empty));
    assert!(!inner.shutdown_token.is_cancelled());
}

#[test]
fn decline_with_single_handler_requests_shutdown() {
    let inner = RouterInner::new(Duration::from_secs(2));
    let (guard, mut rx) = push_handler(&inner);

    inner.notify_below(guard.id);

    assert_eq!(recv_error(&mut rx), Some(TryRecvError::Empty));
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
        .expect("the notification channel stayed open")
        .handled();

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
    // Dropped unresolved: this handler was notified but did nothing with the
    // press, which is what leaves it on the ladder for the next one.
    drop(
        interrupt_rx
            .recv()
            .await
            .expect("handler should be notified"),
    );
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

/// Verify that [`SignalRouter::new`]'s immediate-exit handler restores the
/// terminal when a prompt is open.
///
/// Spawns this test binary in a PTY running
/// [`a_prompt_left_open_until_the_process_exits`], sends `SIGQUIT`, and checks
/// that the cursor moves to the start of the line below the prompt.
#[cfg(unix)]
#[test]
fn an_immediate_exit_gives_back_the_terminal_an_open_prompt_holds() {
    let tmp = tempdir().expect("a temporary directory");
    let pid_file = tmp.path().join("pid");

    let terminal = Terminal::pty(Size::new(10, 80))
        .expect("a pty")
        .with_timeout(PROBE_TIMEOUT);
    let mut command =
        CommandBuilder::new(std::env::current_exe().expect("the path of this test binary"));
    command.args([
        "--exact",
        "signals::tests::a_prompt_left_open_until_the_process_exits",
        "--nocapture",
    ]);
    command.env(EXIT_PROBE_PID_FILE, pid_file.as_str());
    let mut probe = terminal.spawn(command).expect("the probe to start");

    let screen = terminal
        .wait_for("the prompt", |screen| screen.contains("Run it?"))
        .expect("the prompt to be drawn");
    assert_eq!(screen.cursor().0, 0, "the cursor waits on the prompt's row");

    let pid: u32 = fs::read_to_string(&pid_file)
        .expect("the pid file to be written")
        .parse()
        .expect("a pid");

    let sent = Command::new("sh")
        .args(["-c", &format!("kill -QUIT {pid}")])
        .status()
        .expect("a shell to send the signal");
    assert!(sent.success(), "SIGQUIT is sent to the probe: {sent}");

    assert_eq!(
        probe.wait_within(PROBE_TIMEOUT).expect("the exit check"),
        Some(false),
        "the probe exits with the code of a process ended by SIGQUIT"
    );

    let screen = terminal
        .wait_for("the release", |screen| screen.cursor() == (1, 0))
        .expect("the terminal to be given back");
    assert_eq!(screen.used(), ["? Run it? [y,n,?]"], "{screen}");
}

/// Child-process entrypoint for
/// [`an_immediate_exit_gives_back_the_terminal_an_open_prompt_holds`].
///
/// No-op unless [`EXIT_PROBE_PID_FILE`] is set.
/// Starts a [`SignalRouter`], opens an [`InlineSelect`] prompt, and blocks
/// until killed by a signal.
#[cfg(unix)]
#[test]
fn a_prompt_left_open_until_the_process_exits() {
    let Some(pid_file) = std::env::var_os(EXIT_PROBE_PID_FILE) else {
        return;
    };

    let runtime = Runtime::new().expect("a runtime");
    let _router = SignalRouter::new(&runtime, Duration::from_secs(2));

    let mut stdout = io::stdout();
    write!(stdout, "\x1b[2J\x1b[H").expect("the screen to be cleared");
    stdout.flush().expect("the screen to be cleared");

    fs::write(pid_file, std::process::id().to_string()).expect("the pid file to be written");

    let options = vec![
        InlineOption::new('y', "run it"),
        InlineOption::new('n', "skip it"),
    ];
    drop(InlineSelect::new("Run it?", options).prompt(&mut io::stdout()));
}
