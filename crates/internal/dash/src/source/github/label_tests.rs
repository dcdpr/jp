use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use super::*;

#[test]
fn test_label_edits_reach_github_in_the_order_pressed() {
    let applied = Arc::new(Mutex::new(vec![]));
    let (done, finished) = mpsc::channel();

    let edits = {
        let applied = Arc::clone(&applied);
        editor(
            move |number, add| {
                // The add is slow: run side by side, the remove would land first.
                if add {
                    thread::sleep(Duration::from_millis(50));
                }
                applied.lock().unwrap().push((number, add));
                Ok(())
            },
            move |_, _, _| {
                let _gone = done.send(());
            },
        )
    };
    edits.send((7, true)).unwrap();
    edits.send((7, false)).unwrap();
    for _ in 0..2 {
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    assert_eq!(*applied.lock().unwrap(), [(7, true), (7, false)]);
}
