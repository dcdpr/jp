use super::*;

#[test]
fn test_printer_async_ordering() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.print("1");
    printer.print("234".typewriter(Duration::from_millis(10)));
    printer.print("5");

    // Wait for all tasks to complete
    printer.flush();

    assert_eq!(*out.lock(), "12345");
}

#[test]
fn test_printer_targets() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);

    printer.println("Stdout");
    printer.eprintln("Stderr");

    printer.flush();

    assert_eq!(*out.lock(), "Stdout\n");
    assert_eq!(*err.lock(), "Stderr\n");
}

#[test]
fn test_printer_writer() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);

    writeln!(printer.out_writer(), "Hello Writer").unwrap();
    writeln!(printer.err_writer(), "Error Writer").unwrap();

    printer.flush();

    assert_eq!(*out.lock(), "Hello Writer\n");
    assert_eq!(*err.lock(), "Error Writer\n");
}

#[test]
fn test_flush_instant_skips_typewriter_delay() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.print("1");
    // Large delay that would take ~10s if not flushed instantly
    printer.print("234".typewriter(Duration::from_secs(10)));
    printer.print("5");

    // Should complete near-instantly despite the typewriter delay
    let start = std::time::Instant::now();
    printer.flush_instant();
    let elapsed = start.elapsed();

    assert_eq!(*out.lock(), "12345");
    assert!(
        elapsed < Duration::from_secs(1),
        "flush_instant should skip typewriter delays, took {elapsed:?}"
    );
}

#[test]
fn test_flush_instant_honors_pending_flush() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.print("hello");
    // A regular flush is queued but hasn't been waited on
    let (flush_tx, flush_rx) = mpsc::channel();
    printer.send(Command::Flush(flush_tx));
    printer.print(" world");

    printer.flush_instant();

    // The pending Flush should have been signaled during drain
    assert!(
        flush_rx.try_recv().is_ok(),
        "pending Flush should be signaled during flush_instant"
    );
    assert_eq!(*out.lock(), "hello world");
}

#[test]
fn test_flush_instant_with_no_pending_tasks() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.print("already sent");
    printer.flush();

    // Nothing pending — should be a no-op
    printer.flush_instant();

    assert_eq!(*out.lock(), "already sent");
}

#[test]
fn test_pretty_false_strips_ansi() {
    let (printer, out, _) = Printer::memory(OutputFormat::Text);

    printer.print("\x1b[32mgreen\x1b[0m plain");
    printer.flush();

    assert_eq!(*out.lock(), "green plain");
}

#[test]
fn test_pretty_true_preserves_ansi() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.print("\x1b[32mgreen\x1b[0m plain");
    printer.flush();

    assert_eq!(*out.lock(), "\x1b[32mgreen\x1b[0m plain");
}

#[test]
fn test_pretty_false_strips_typewriter_ansi() {
    let (printer, out, _) = Printer::memory(OutputFormat::Text);

    printer.print("\x1b[1mbold\x1b[0m".typewriter(Duration::ZERO));
    printer.flush();

    assert_eq!(*out.lock(), "bold");
}

#[test]
fn test_default_format_strips_ansi() {
    let (printer, out, _) = Printer::memory(OutputFormat::default());

    printer.print("\x1b[31mred\x1b[0m");
    printer.flush();

    // Default is Text (not pretty), so ANSI is stripped.
    assert_eq!(*out.lock(), "red");
}

#[test]
fn split_ansi_across_tasks_is_stripped() {
    // Regression for issue 683. `writeln!` + crossterm emit a single SGR
    // sequence across several `write_str` calls, and each call becomes its own
    // print task. If stripping doesn't persist parser state across tasks, the
    // CSI introducers get dropped while the parameter bytes survive, producing
    // cruft like `38;5;11m1mgit_diff`.
    let (printer, out, _) = Printer::memory(OutputFormat::Text);

    let pieces = [
        "\x1b[", "38;", "5;11", "m", "\x1b[", "1", "m", "git_diff", "\x1b[0m",
    ];
    for piece in pieces {
        printer.print(piece);
    }
    printer.flush();

    assert_eq!(*out.lock(), "git_diff");
}

#[test]
fn ansi_state_is_independent_per_stream() {
    // A sequence left open on one stream must not consume bytes destined for
    // the other: each stream needs its own parser state.
    let (printer, out, err) = Printer::memory(OutputFormat::Text);

    printer.print("\x1b[");
    printer.eprint("\x1b[");
    printer.print("31mred");
    printer.eprint("32mgreen");
    printer.flush();

    assert_eq!(*out.lock(), "red");
    assert_eq!(*err.lock(), "green");
}

#[test]
fn json_println_wraps_in_ndjson() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.println("hello world");
    printer.flush();

    let output = out.lock().clone();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["message"], "hello world");
}

#[test]
fn json_print_wraps_in_ndjson() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.print("partial content");
    printer.flush();

    let output = out.lock().clone();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["message"], "partial content");
}

#[test]
fn json_pretty_println_is_indented() {
    let (printer, out, _) = Printer::memory(OutputFormat::JsonPretty);

    printer.println("test");
    printer.flush();

    let output = out.lock().clone();
    assert!(
        output.contains("\n  "),
        "expected indented JSON, got: {output}"
    );
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["message"], "test");
}

#[test]
fn println_raw_bypasses_json_wrapping() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.println_raw("{\"custom\":true}");
    printer.flush();

    let output = out.lock().clone();
    assert_eq!(output, "{\"custom\":true}\n");
}

#[test]
fn text_println_unchanged() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    printer.println("just text");
    printer.flush();

    assert_eq!(*out.lock(), "just text\n");
}

#[test]
fn json_println_strips_ansi_before_wrapping() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.println("\x1b[32mgreen\x1b[0m plain");
    printer.flush();

    let output = out.lock().clone();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["message"], "green plain");
}

#[test]
fn json_print_strips_ansi_before_wrapping() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.print("\x1b[48;5;236m\x1b[1m**Bold**\x1b[22m\x1b[K\x1b[0m");
    printer.flush();

    let output = out.lock().clone();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["message"], "**Bold**");
}

#[test]
fn effective_delay_returns_cap_when_disabled() {
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(100),
        max_latency_nanos: AtomicU64::new(0),
        drain_snapshot: AtomicUsize::new(0),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_millis(3),
    );
}

#[test]
fn effective_delay_returns_cap_when_pending_is_zero() {
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(0),
        max_latency_nanos: AtomicU64::new(500_000_000),
        drain_snapshot: AtomicUsize::new(0),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_millis(3),
    );
}

#[test]
fn effective_delay_clamps_to_cap_when_queue_is_small() {
    // 500ms budget / 10 pending = 50ms per char, but cap is 3ms.
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(10),
        max_latency_nanos: AtomicU64::new(500_000_000),
        drain_snapshot: AtomicUsize::new(0),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_millis(3),
    );
}

#[test]
fn effective_delay_accelerates_when_queue_is_large() {
    // 500ms budget / 1000 pending = 500us per char, under the 3ms cap.
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(1000),
        max_latency_nanos: AtomicU64::new(500_000_000),
        drain_snapshot: AtomicUsize::new(0),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_micros(500),
    );
}

#[test]
fn effective_delay_in_drain_mode_uses_snapshot_floor() {
    // Drain snapshot was taken at 1000 pending. Pending has since dropped
    // to 50, but the controller still divides by 1000 (so delay stays at
    // 500us instead of rising back toward the cap).
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(50),
        max_latency_nanos: AtomicU64::new(500_000_000),
        drain_snapshot: AtomicUsize::new(1000),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_micros(500),
    );
}

#[test]
fn effective_delay_in_drain_mode_can_speed_up_further() {
    // Drain snapshot at 100, but in the meantime more typewriter content
    // arrived and pushed pending to 1000. Since denom = max(snapshot,
    // pending), the controller speeds up to the new floor rather than
    // sticking with the old drain pace. This codifies that any new
    // typewriter task is also expected to clear the snapshot via
    // `track_pending` — the max() is just a safety net.
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(1000),
        max_latency_nanos: AtomicU64::new(500_000_000),
        drain_snapshot: AtomicUsize::new(100),
    };

    assert_eq!(
        dc.effective_delay(Duration::from_millis(3)),
        Duration::from_micros(500),
    );
}

#[test]
fn visible_char_count_ignores_ansi_and_control() {
    assert_eq!(visible_char_count("hello"), 5);
    assert_eq!(visible_char_count("\x1b[1mhello\x1b[0m"), 5);
    assert_eq!(visible_char_count("\x1b[1m\x1b[31m\x1b[0m"), 0);
    assert_eq!(visible_char_count("a\nb"), 2);
}

#[test]
fn release_pending_saturates_at_zero() {
    let dc = DelayControl {
        skip: Mutex::new(false),
        wake: Condvar::new(),
        pending_chars: AtomicUsize::new(3),
        max_latency_nanos: AtomicU64::new(0),
        drain_snapshot: AtomicUsize::new(0),
    };

    release_pending(&dc, 10);
    assert_eq!(dc.pending_chars.load(Ordering::Relaxed), 0);
}

#[test]
fn mark_typewriter_drained_snapshots_pending() {
    let printer = Printer::sink();
    printer.set_max_latency(Duration::from_millis(500));

    // Queue typewriter content without letting the worker drain it: use
    // a delay so big the worker would still be on the first character.
    printer.print("hello".typewriter(Duration::from_mins(1)));
    printer.mark_typewriter_drained();

    assert!(
        printer.delay_control.drain_snapshot.load(Ordering::Relaxed) > 0,
        "drain snapshot must capture the pending count",
    );

    // Drain via flush_instant so the test doesn't hang on the 60s sleep.
    printer.flush_instant();
}

#[test]
fn new_typewriter_task_clears_drain_snapshot() {
    let printer = Printer::sink();
    printer.set_max_latency(Duration::from_millis(500));

    printer.print("hello".typewriter(Duration::from_mins(1)));
    printer.mark_typewriter_drained();
    assert!(printer.delay_control.drain_snapshot.load(Ordering::Relaxed) > 0);

    // Any new typewriter enqueue should reset the snapshot: the producer
    // is no longer idle.
    printer.print("more".typewriter(Duration::from_mins(1)));
    assert_eq!(
        printer.delay_control.drain_snapshot.load(Ordering::Relaxed),
        0,
    );

    printer.flush_instant();
}

#[test]
fn track_pending_ignores_instant_tasks() {
    let printer = Printer::sink();
    printer.set_max_latency(Duration::from_millis(500));

    printer.print("no counter for me");
    printer.flush();

    assert_eq!(
        printer.delay_control.pending_chars.load(Ordering::Relaxed),
        0,
    );
}

#[test]
fn bounded_latency_controller_drains_large_queue_quickly() {
    // Without the controller, 200 chars at 50ms cap would take ~10s.
    // With max_latency=200ms and 200 chars: delay = min(50ms, 200ms/200) =
    // 1ms. Total ≈ 200ms. Allow generous slack for slow CI machines.
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    printer.set_max_latency(Duration::from_millis(200));

    let content = "x".repeat(200);
    let start = std::time::Instant::now();
    printer.print(content.typewriter(Duration::from_millis(50)));
    printer.flush();
    let elapsed = start.elapsed();

    assert_eq!(out.lock().len(), 200);
    assert!(
        elapsed < Duration::from_secs(2),
        "bounded-latency controller should drain in well under 2s, took {elapsed:?}"
    );
}

#[test]
fn static_behavior_preserved_when_max_latency_unset() {
    // Without max_latency, the worker uses the per-task cap directly.
    // 20 chars at 5ms = ~100ms; 0 max_latency (the default) means no speed-up.
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);

    let content = "y".repeat(20);
    let start = std::time::Instant::now();
    printer.print(content.typewriter(Duration::from_millis(5)));
    printer.flush();
    let elapsed = start.elapsed();

    assert_eq!(out.lock().len(), 20);
    assert!(
        elapsed >= Duration::from_millis(50),
        "static-delay behavior must be preserved (no controller speed-up); elapsed {elapsed:?}",
    );
}
