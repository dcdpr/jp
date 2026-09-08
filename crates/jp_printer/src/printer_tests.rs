use super::*;
use crate::region::OutputLines;

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

// Chrome on stderr is NDJSON too under `--format json` (RFD 048), so a
// consumer merging the streams gets one record shape rather than two.
#[test]
fn json_eprintln_wraps_in_ndjson() {
    let (printer, _, err) = Printer::memory(OutputFormat::Json);

    printer.eprintln("\x1b[33mnote: using workspace `A`\x1b[0m");
    printer.flush();

    assert_eq!(*err.lock(), "{\"message\":\"note: using workspace `A`\"}\n");
}

// The reasoning-progress indicator emits a bare `.` per chunk. Each becomes
// its own record rather than a fragment: noise a consumer can filter beats a
// line that makes `jq` give up on the whole stream.
#[test]
fn json_eprint_wraps_each_fragment_as_a_record() {
    let (printer, _, err) = Printer::memory(OutputFormat::Json);

    printer.eprint(".");
    printer.eprint(".");
    printer.flush();

    assert_eq!(*err.lock(), "{\"message\":\".\"}\n{\"message\":\".\"}\n");
}

#[test]
fn erase_line_writes_the_escape_on_a_text_format() {
    let (printer, _, err) = Printer::memory(OutputFormat::TextPretty);

    printer.erase_line();
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[K");
}

// A cursor escape is neither a record nor part of one, so emitting it into an
// NDJSON stream leaves the stream unparseable for the sake of a repaint no
// JSON consumer can see.
#[test]
fn erase_line_is_silent_in_json() {
    let (printer, _, err) = Printer::memory(OutputFormat::Json);

    printer.erase_line();
    printer.flush();

    assert_eq!(*err.lock(), "");
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

/// A memory printer that behaves as if stderr were an 80-column terminal.
fn region_printer() -> (Printer, SharedBuffer, SharedBuffer) {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_terminal(TerminalCapability::interactive(Some(80)));

    (printer, out, err)
}

/// A region that is due the moment it is claimed.
fn waiting_style() -> RegionStyle {
    RegionStyle::new(Duration::ZERO, Duration::from_millis(10), |_, detail| {
        detail.map_or_else(|| "waiting".to_owned(), |d| format!("waiting: {d}"))
    })
}

#[test]
fn a_claimed_region_draws_on_the_chrome_channel() {
    let (printer, out, err) = region_printer();

    let region = printer.status_region(waiting_style());
    printer.flush();

    assert!(region.is_active());
    assert_eq!(*err.lock(), "\r\x1b[Kwaiting");
    assert!(out.lock().is_empty(), "a region is chrome, never stdout");
}

#[test]
fn regions_are_inert_without_an_interactive_stderr() {
    // The default capability models a piped stderr, which is what every
    // memory and sink printer gets.
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);

    let region = printer.status_region(waiting_style());
    region.set_detail("still nothing");
    printer.flush();

    assert!(!region.is_active());
    assert!(err.lock().is_empty());
}

#[test]
fn regions_are_inert_in_a_non_pretty_format() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);
    let printer = printer.with_terminal(TerminalCapability::interactive(Some(80)));

    let region = printer.status_region(waiting_style());
    printer.flush();

    assert!(!region.is_active());
    assert!(err.lock().is_empty());
}

#[test]
fn regions_are_inert_while_logs_go_to_stderr() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer
        .with_terminal(TerminalCapability::interactive(Some(80)))
        .with_stderr_logging(true);

    let region = printer.status_region(waiting_style());
    printer.flush();

    assert!(!region.is_active());
    assert!(err.lock().is_empty());
}

#[test]
fn dropping_the_handle_erases_the_row() {
    let (printer, _out, err) = region_printer();

    let region = printer.status_region(waiting_style());
    printer.flush();
    drop(region);
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[Kwaiting\r\x1b[K");
}

#[test]
fn chrome_writes_erase_the_region_and_redraw_after_it() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    printer.eprintln("a persistent line");
    printer.flush();

    assert_eq!(
        *err.lock(),
        "\r\x1b[Kwaiting\r\x1b[Ka persistent line\n\r\x1b[Kwaiting"
    );
}

#[test]
fn stdout_writes_erase_the_region_and_redraw_after_it() {
    let (printer, out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    printer.println("an answer");
    printer.flush();

    assert_eq!(*out.lock(), "an answer\n");
    assert_eq!(*err.lock(), "\r\x1b[Kwaiting\r\x1b[K\r\x1b[Kwaiting");
}

#[test]
fn a_row_is_not_painted_over_an_unfinished_line() {
    // `write!` sends one task per `write_str`, so `writeln!(w, "see: {}", path)`
    // arrives as three. A row painted after the first would start with
    // `\r\x1b[K` and erase the text it was meant to sit below.
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    err.lock().clear();

    let mut writer = printer.err_writer();
    write!(writer, "see: ").unwrap();
    printer.flush();
    assert_eq!(
        *err.lock(),
        "\r\x1b[Ksee: ",
        "the row is erased for the write and not redrawn over it"
    );

    write!(writer, "/tmp/result.txt").unwrap();
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[Ksee: /tmp/result.txt");

    // The newline finishes the line, so the row is free to come back.
    writeln!(writer).unwrap();
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[Ksee: /tmp/result.txt\n\r\x1b[Kwaiting");
}

#[test]
fn an_unfinished_line_stops_the_region_ticking() {
    // Waking on the interval to paint nothing is work for no one; the worker
    // blocks until a later write finishes the line.
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(RegionStyle::new(
        Duration::ZERO,
        Duration::from_millis(10),
        |secs, _| format!("{secs:.1}s"),
    ));
    printer.flush();

    write!(printer.err_writer(), "mid-line").unwrap();
    printer.flush();
    err.lock().clear();

    thread::sleep(Duration::from_millis(150));
    printer.flush();

    assert!(
        err.lock().is_empty(),
        "no frame may land while the cursor sits mid-row: {:?}",
        *err.lock()
    );
}

#[test]
fn an_empty_print_leaves_the_region_alone() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    printer.print("");
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[Kwaiting");
}

#[test]
fn setting_a_detail_repaints_the_row() {
    let (printer, _out, err) = region_printer();

    let region = printer.status_region(waiting_style());
    printer.flush();
    region.detail().set("bookworm");
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[Kwaiting\r\x1b[Kwaiting: bookworm");
}

#[test]
fn releasing_a_shaded_region_leaves_no_paint_behind() {
    let (printer, _out, err) = region_printer();

    let region = printer.status_region(waiting_style());
    region.background().set("48;5;236");
    printer.flush();
    drop(region);
    printer.flush();

    // The row is shaded while the region owns it, and the release clears it to
    // the terminal default. A background on the erase would paint the row to
    // the right edge instead, and whatever wrote there next — including the
    // shell prompt after `jp` exits — would sit in front of that paint.
    assert_eq!(
        *err.lock(),
        "\r\x1b[Kwaiting\r\x1b[48;5;236m\x1b[Kwaiting\x1b[49m\r\x1b[K"
    );
}

#[test]
fn the_region_ticks_on_its_own() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(RegionStyle::new(
        Duration::ZERO,
        Duration::from_millis(10),
        |secs, _| format!("{secs:.1}s"),
    ));
    thread::sleep(Duration::from_millis(150));
    printer.flush();

    let frames = err.lock().matches("\r\x1b[K").count();
    assert!(
        frames >= 2,
        "the worker must redraw without being prompted; saw {frames} frame(s)"
    );
}

#[test]
fn a_delayed_region_stays_hidden() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(RegionStyle::new(
        Duration::from_mins(1),
        Duration::from_millis(10),
        |_, _| "too soon".to_owned(),
    ));
    printer.flush();

    assert!(err.lock().is_empty());
}

#[test]
fn claims_stack_and_releasing_the_top_re_exposes_the_one_below() {
    let (printer, _out, err) = region_printer();

    let _outer = printer.status_region(waiting_style());
    printer.flush();

    let inner = printer.status_region(RegionStyle::new(
        Duration::ZERO,
        Duration::from_millis(10),
        |_, _| "running tool".to_owned(),
    ));
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[Kwaiting\r\x1b[K\r\x1b[Krunning tool");

    err.lock().clear();
    drop(inner);
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[K\r\x1b[Kwaiting");
}

#[test]
fn acquiring_a_prompt_writer_drains_the_queue_first() {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);

    // A per-character delay long enough that the worker is certainly still
    // inside the task, standing in for the stream output a tool call's chrome
    // is queued behind.
    printer.print("queued content\n".typewriter(Duration::from_secs(10)));

    // No flush: acquisition is the barrier. A widget changes the terminal mode
    // and takes the cursor directly, neither of which travels through the
    // printer's queue, so anything still in it would land on a terminal the
    // widget has already reconfigured — line feeds with no carriage return,
    // under a cursor the widget believes it owns.
    let start = Instant::now();
    let _prompt = printer.prompt_writer();
    let elapsed = start.elapsed();

    assert_eq!(*out.lock(), "queued content\n");
    assert!(
        elapsed < Duration::from_secs(1),
        "acquisition must skip typewriter delays, took {elapsed:?}"
    );
}

#[test]
fn acquiring_an_owned_prompt_writer_drains_the_queue_first() {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);

    printer.print("queued content\n".typewriter(Duration::from_secs(10)));

    let _prompt = printer.owned_prompt_writer();

    assert_eq!(*out.lock(), "queued content\n");
}

#[test]
fn a_prompt_writer_erases_the_region_before_it_returns() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    err.lock().clear();

    // No flush, for the same reason `suspend_status` blocks: the rows have to
    // be gone by the time the widget paints, and the widget does not paint
    // through the printer.
    let _prompt = printer.prompt_writer();
    assert_eq!(*err.lock(), "\r\x1b[K");
}

#[test]
fn a_prompt_writer_suspends_the_region_for_its_lifetime() {
    let (printer, _out, err) = region_printer();

    let region = printer.status_region(waiting_style());
    printer.flush();
    err.lock().clear();

    {
        let _prompt = printer.prompt_writer();
        printer.flush();
        // Nothing repaints while a widget owns the cursor, not even a detail
        // update the client pushes mid-prompt.
        region.detail().set("bookworm");
        printer.flush();
        assert_eq!(*err.lock(), "\r\x1b[K");
    }

    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[K\r\x1b[Kwaiting: bookworm");
}

#[test]
fn a_prompt_that_ends_mid_line_does_not_hold_the_region_hostage() {
    // A prompt widget owns the cursor and routinely leaves it mid-row. That is
    // the widget's business, not the region's: the suspension is what kept the
    // rows away, and when it lifts the region starts fresh. Treating the
    // prompt's last write as unfinished content keeps the region hidden until
    // something newline-terminated happens by, which may be a whole tool
    // execution later.
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    err.lock().clear();

    {
        let mut prompt = printer.prompt_writer();
        write!(prompt, "Deliver result? [y/n] ").unwrap();
        printer.flush();
    }

    printer.flush();
    assert!(
        err.lock().ends_with("waiting"),
        "the row must return when the prompt releases the terminal, got {:?}",
        *err.lock()
    );
}

#[test]
fn suspend_status_erases_before_it_returns() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    err.lock().clear();

    // No flush: the guard only returns once the worker has applied the
    // suspension, which is what makes it safe to hand the terminal to a child
    // process.
    let guard = printer.suspend_status();
    assert_eq!(*err.lock(), "\r\x1b[K");

    drop(guard);
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[K\r\x1b[Kwaiting");
}

#[test]
fn shutdown_erases_the_region() {
    let (printer, _out, err) = region_printer();

    let _region = printer.status_region(waiting_style());
    printer.flush();
    printer.shutdown();

    assert_eq!(*err.lock(), "\r\x1b[Kwaiting\r\x1b[K");
}

#[test]
fn a_burst_of_pushes_shows_only_the_newest_lines() {
    const PUSHES: usize = 500;

    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer =
        printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24)));

    let region = printer.status_region(
        RegionStyle::new(Duration::ZERO, Duration::from_mins(1), |_, _| {
            "* status".to_owned()
        })
        .with_output(OutputLines::Rows(2)),
    );
    printer.flush();
    err.lock().clear();

    let sink = region.source("build");
    for n in 1..=PUSHES {
        sink.push(format!("line {n}"));
    }
    printer.flush();

    // The buffer holds far more than the window shows, and the worker paints
    // whenever it gets scheduled, so `err` accumulates a frame per paint rather
    // than one screen. How many, and which lines each caught mid-burst, is the
    // scheduler's business.
    //
    // What has to hold is the paint that lands last: the window is a tail of
    // the buffer, so it shows the newest two lines and nothing older. The tick
    // interval is a minute out, so that frame came from a refresh.
    let chrome = err.lock().clone();
    let last = format!(
        "\x1b[2A\r\x1b[Kline {}\n\r\x1b[Kline {PUSHES}\n\r\x1b[K* status",
        PUSHES - 1
    );
    assert!(
        chrome.ends_with(&last),
        "the last frame must be {last:?}, got {chrome:?}"
    );
}

#[test]
fn rows_are_bounded_to_the_terminal_width() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_terminal(TerminalCapability::interactive(Some(8)));

    let _region = printer.status_region(RegionStyle::new(
        Duration::ZERO,
        Duration::from_millis(10),
        |_, _| "far too wide for this terminal".to_owned(),
    ));
    printer.flush();

    assert_eq!(*err.lock(), "\r\x1b[Kfar too ");
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

// --- chrome ------------------------------------------------------------------

// A silenced channel drops chrome and nothing else: command output and the
// assistant's response are data, and keep their stream.
#[test]
fn silenced_chrome_drops_stderr_and_keeps_stdout() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    printer.println("data");
    printer.eprintln("a status line");
    printer.eprint(".");
    printer.flush();

    assert_eq!(*out.lock(), "data\n");
    assert_eq!(*err.lock(), "");
}

// Writer output reaches the worker without passing `Printer::send`, so it
// needs its own guard. The retry notice and the status timer both write this
// way.
#[test]
fn silenced_chrome_drops_writer_output() {
    let (printer, _, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    writeln!(printer.err_writer(), "waiting 3s").unwrap();
    printer.erase_line();
    printer.flush();

    assert_eq!(*err.lock(), "");
}

// Renderers hold clones of the printer, and a clone that printed chrome would
// reopen the channel the flag closed.
#[test]
fn silenced_chrome_survives_a_clone() {
    let (printer, _, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    let renderer = printer.clone();
    renderer.eprintln("a status line");
    printer.flush();

    assert_eq!(*err.lock(), "");
}

// Stdout is not chrome, so a silenced channel leaves the writer over it alone.
#[test]
fn silenced_chrome_leaves_the_out_writer_alone() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    writeln!(printer.out_writer(), "data").unwrap();
    printer.flush();

    assert_eq!(*out.lock(), "data\n");
}

// A question's own context is not chrome. Silencing chrome must not leave a run
// blocking on a question whose subject it withheld, which is what happens when
// the identity of a binary awaiting approval travels as chrome.
// A memory printer has no tty, so the prompt stream falls back to stdout.
#[test]
fn silenced_chrome_leaves_the_prompt_channel_alone() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    printer.prompt_println("  \u{2192} Found jp-serve on $PATH (/usr/local/bin/jp-serve)");
    printer.eprintln("a status line");
    printer.flush();

    assert_eq!(
        *out.lock(),
        "  \u{2192} Found jp-serve on $PATH (/usr/local/bin/jp-serve)\n"
    );
    assert_eq!(*err.lock(), "");
}

// A prompt is not data, so it carries no JSON envelope even when the run's
// output format is JSON.
#[test]
fn a_prompt_line_is_never_wrapped_as_json() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);

    printer.prompt_println("Install it?");
    printer.flush();

    assert_eq!(*out.lock(), "Install it?\n");
}

// A repaint needs somewhere to land and someone to see it. Callers asking
// whether to do the work behind one get both answers from one question.
#[test]
fn chrome_repaints_only_when_shown_and_not_json() {
    let (text, _, _) = Printer::memory(OutputFormat::TextPretty);
    let (json, _, _) = Printer::memory(OutputFormat::Json);
    let (silenced, _, _) = Printer::memory(OutputFormat::TextPretty);
    let silenced = silenced.with_chrome(Chrome::Silenced);

    assert!(text.chrome_repaints());
    assert!(!json.chrome_repaints());
    assert!(!silenced.chrome_repaints());
}
