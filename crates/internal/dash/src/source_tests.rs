use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::*;

#[test]
fn test_parallel_keeps_the_order_of_its_items() {
    let items: Vec<u64> = (0..50).collect();

    let doubled = parallel(&items, 8, |n| {
        // Later items finish first, so arrival order differs from item order.
        thread::sleep(Duration::from_micros(50 - n));
        n * 2
    });

    assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
}

#[test]
fn test_parallel_runs_its_items_at_the_same_time() {
    let running = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);

    parallel(&[(); 4], 4, |()| {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);

        // Wait for the other three to start; one after another, they never do.
        let deadline = Instant::now() + Duration::from_secs(5);
        while running.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
            thread::yield_now();
        }
        peak.fetch_max(running.load(Ordering::SeqCst), Ordering::SeqCst);

        // Stay counted until all four have looked, then leave.
        while peak.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
            thread::yield_now();
        }
        running.fetch_sub(1, Ordering::SeqCst);
    });

    assert_eq!(peak.load(Ordering::SeqCst), 4);
}

#[test]
fn test_parallel_stays_within_its_worker_limit() {
    let running = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);

    parallel(&[(); 40], 3, |()| {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(1));
        running.fetch_sub(1, Ordering::SeqCst);
    });

    assert!(peak.load(Ordering::SeqCst) <= 3);
}

#[test]
fn test_parallel_with_no_items() {
    assert_eq!(parallel(&[] as &[u8], 8, |n| *n), Vec::<u8>::new());
}
