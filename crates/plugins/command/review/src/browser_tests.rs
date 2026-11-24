#[cfg(unix)]
use std::time::Instant;

use pretty_assertions::assert_eq;

use super::*;

#[cfg(unix)]
fn shell(script: &str) -> Child {
    Command::new("sh").args(["-c", script]).spawn().unwrap()
}

#[cfg(unix)]
#[test]
fn an_opener_that_exits_reports_how() {
    assert_eq!(settle(shell("exit 0"), Duration::from_secs(5)), Ok(()));
    assert_eq!(
        settle(shell("exit 3"), Duration::from_secs(5)),
        Err("exit status: 3".to_owned())
    );
}

/// `xdg-open` blocks while it runs a browser in the foreground; the server
/// opening a page has to go on answering meanwhile.
#[cfg(unix)]
#[test]
fn an_opener_still_running_after_the_grace_period_is_left_to_run() {
    let started = Instant::now();

    let settled = settle(shell("sleep 3"), Duration::from_millis(100));

    assert_eq!(settled, Ok(()));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited {:?} for an opener still running",
        started.elapsed()
    );
}

#[test]
fn each_system_opens_a_url_its_own_way() {
    let url = "http://127.0.0.1:3001/reviews/R-1h4k2m9";

    assert_eq!(opener("macos", url), Some(("open", vec![url.to_owned()])));
    assert_eq!(
        opener("linux", url),
        Some(("xdg-open", vec![url.to_owned()]))
    );
    assert_eq!(
        opener("windows", url),
        Some(("rundll32", vec![
            "url.dll,FileProtocolHandler".to_owned(),
            url.to_owned()
        ]))
    );
    assert_eq!(opener("wasi", url), None);
}
