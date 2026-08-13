use std::{sync::Arc, time::Duration};

use jp_printer::{OutputFormat, Printer};

use super::*;

fn test_printer() -> (Arc<Printer>, jp_printer::SharedBuffer) {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    (Arc::new(printer), err)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_line_timer_renders_without_status() {
    let (printer, err) = test_printer();

    let timer = spawn_line_timer(
        printer.clone(),
        true,
        Duration::ZERO,
        Duration::from_millis(20),
        |secs, status| match status {
            Some(detail) => format!("\r\x1b[Ktick {secs:.1}s ({detail})"),
            None => format!("\r\x1b[Ktick {secs:.1}s"),
        },
    )
    .expect("show = true spawns a timer");

    tokio::time::sleep(Duration::from_millis(100)).await;
    timer.finish().await;
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("tick"),
        "Timer should have rendered at least one tick.\nChrome:\n{chrome}"
    );
    assert!(
        !chrome.contains('('),
        "No status was set; no detail should render.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "Finish must leave the line cleared.\nChrome:\n{chrome}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_line_timer_set_status_redraws_with_detail() {
    let (printer, err) = test_printer();

    let timer = spawn_line_timer(
        printer.clone(),
        true,
        Duration::ZERO,
        // Long interval: the status detail below can only appear via the
        // status-change redraw, not via a scheduled tick.
        Duration::from_mins(1),
        |secs, status| match status {
            Some(detail) => format!("\r\x1b[Ktick {secs:.1}s ({detail})"),
            None => format!("\r\x1b[Ktick {secs:.1}s"),
        },
    )
    .expect("show = true spawns a timer");

    // Let the first (immediate) tick land before pushing a status.
    tokio::time::sleep(Duration::from_millis(50)).await;
    timer.set_status("receiving data");
    tokio::time::sleep(Duration::from_millis(50)).await;
    timer.finish().await;
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("(receiving data)"),
        "Status change should redraw immediately with the detail.\nChrome:\n{chrome}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_line_timer_show_false_spawns_nothing() {
    let (printer, err) = test_printer();

    let timer = spawn_line_timer(
        printer.clone(),
        false,
        Duration::ZERO,
        Duration::from_millis(10),
        |secs, _status| format!("tick {secs:.1}s"),
    );
    assert!(timer.is_none());

    tokio::time::sleep(Duration::from_millis(50)).await;
    printer.flush();
    assert!(err.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_line_timer_drop_cancels_task() {
    let (printer, err) = test_printer();

    let timer = spawn_line_timer(
        printer.clone(),
        true,
        Duration::ZERO,
        Duration::from_millis(10),
        |secs, _status| format!("\r\x1b[Ktick {secs:.1}s"),
    )
    .expect("show = true spawns a timer");

    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(timer);
    // Give the cancelled task time to observe the token and clear its line.
    tokio::time::sleep(Duration::from_millis(50)).await;
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "Dropped timer should stop ticking and clear its line.\nChrome:\n{chrome}"
    );
}
