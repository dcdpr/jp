use std::thread;

use super::*;

#[test]
fn nothing_is_to_close_outside_a_closer() {
    assert!(!close_requested());
}

#[test]
fn a_closed_closer_closes_the_prompts_opened_under_it() {
    let closer = PromptCloser::new();

    let (before, after) = closing_on(&closer, || {
        let before = close_requested();
        closer.close();
        (before, close_requested())
    });

    assert!(!before);
    assert!(after);
    assert!(!close_requested());
}

#[test]
fn a_closer_closed_beforehand_closes_the_prompts_opened_later() {
    let closer = PromptCloser::new();
    closer.close();

    assert!(closing_on(&closer, close_requested));
}

#[test]
fn an_inner_closer_replaces_the_outer_one_until_it_ends() {
    let outer = PromptCloser::new();
    let inner = PromptCloser::new();
    outer.close();

    let (inside, outside) = closing_on(&outer, || {
        let inside = closing_on(&inner, close_requested);
        (inside, close_requested())
    });

    assert!(!inside);
    assert!(outside);
}

#[test]
fn a_closer_does_not_cover_another_thread() {
    let closer = PromptCloser::new();
    closer.close();

    let elsewhere = closing_on(&closer, || thread::spawn(close_requested).join().unwrap());

    assert!(!elsewhere);
}

#[test]
fn a_watch_is_polled_from_any_thread() {
    let closer = PromptCloser::new();
    let watch = closing_on(&closer, CloseWatch::current);
    closer.close();

    assert!(thread::spawn(move || watch.requested()).join().unwrap());
}
