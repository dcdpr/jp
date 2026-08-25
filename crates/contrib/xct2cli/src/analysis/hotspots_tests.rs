use super::{Hotspot, by_samples};
use crate::address::RuntimePc;

fn hotspot(pc: u64, samples: u64) -> Hotspot {
    Hotspot {
        pc: RuntimePc::new(pc),
        samples,
        fmt: None,
        function: None,
        file: None,
        line: None,
    }
}

fn ranked(mut hotspots: Vec<Hotspot>) -> Vec<(u64, u64)> {
    hotspots.sort_by(by_samples);

    hotspots
        .iter()
        .map(|hotspot| (hotspot.pc.raw(), hotspot.samples))
        .collect()
}

#[test]
fn the_busiest_counter_ranks_first() {
    assert_eq!(
        ranked(vec![
            hotspot(0x1000, 3),
            hotspot(0x2000, 90),
            hotspot(0x3000, 12)
        ]),
        vec![(0x2000, 90), (0x3000, 12), (0x1000, 3)]
    );
}

/// The defect this pins.
/// Counts are accumulated in a `HashMap`, so the order two counters tied on
/// samples arrive in differs between processes.
/// Ranking on the count alone leaves that arbitrary order in place, and
/// truncating to the busiest N then keeps an arbitrary subset of the tie: two
/// reads of the same trace report different frames, and each looks perfectly
/// plausible.
#[test]
fn a_tie_ranks_the_same_whichever_order_it_arrives_in() {
    let one = vec![
        hotspot(0x4000, 1),
        hotspot(0x1000, 1),
        hotspot(0x3000, 1),
        hotspot(0x2000, 1),
    ];
    let other = vec![
        hotspot(0x2000, 1),
        hotspot(0x3000, 1),
        hotspot(0x1000, 1),
        hotspot(0x4000, 1),
    ];

    assert_eq!(ranked(one.clone()), ranked(other.clone()));
    assert_eq!(ranked(one), vec![
        (0x1000, 1),
        (0x2000, 1),
        (0x3000, 1),
        (0x4000, 1)
    ]);
}

/// What the ordering is for: the busiest 2 of a long tail have to be the same 2
/// every time, or a report cannot be compared with itself.
#[test]
fn truncating_a_tail_keeps_the_same_counters_every_time() {
    let tail = |order: [u64; 5]| {
        let mut ranked = ranked(order.iter().map(|pc| hotspot(*pc, 1)).collect());
        ranked.truncate(2);
        ranked
    };

    assert_eq!(tail([0x5000, 0x1000, 0x4000, 0x2000, 0x3000]), vec![
        (0x1000, 1),
        (0x2000, 1)
    ]);
    assert_eq!(tail([0x3000, 0x2000, 0x1000, 0x5000, 0x4000]), vec![
        (0x1000, 1),
        (0x2000, 1)
    ]);
}
