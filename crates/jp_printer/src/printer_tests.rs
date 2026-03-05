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
