use super::*;

#[test]
fn a_page_reloads_once_the_server_names_another_build() {
    assert!(rebuilt("1834927105", "2290315521"));
}

#[test]
fn a_page_stays_while_the_build_it_started_is_current() {
    assert!(!rebuilt("1834927105", "1834927105"));
}

/// `just build-ui` clears the directory before it writes the next build.
#[test]
fn a_page_stays_while_no_build_is_named() {
    assert!(!rebuilt("1834927105", ""));
}

/// A page loaded while a build was being written loaded whatever was there.
#[test]
fn a_page_started_without_a_build_reloads_once_there_is_one() {
    assert!(rebuilt("", "1834927105"));
}

#[test]
fn an_idle_page_polls_every_two_seconds() {
    assert!(due(0.0, None, false), "the first poll");
    assert!(!due(1999.0, Some(0.0), false));
    assert!(due(2000.0, Some(0.0), false));
}

#[test]
fn a_page_with_a_pass_running_polls_every_half_second() {
    assert!(!due(499.0, Some(0.0), true));
    assert!(due(500.0, Some(0.0), true));
}

#[test]
fn a_pass_ended_when_it_stopped_running() {
    assert_eq!(ended(Some(2), None), Some(2));
    assert_eq!(ended(Some(2), Some(3)), Some(2), "and the next one started");
    assert_eq!(ended(Some(2), Some(2)), None);
    assert_eq!(ended(None, Some(1)), None);
}
