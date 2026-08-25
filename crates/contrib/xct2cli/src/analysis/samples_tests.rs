use super::{PcSample, by_samples};
use crate::address::RuntimePc;

fn sample(pc: u64, samples: u64) -> PcSample {
    PcSample {
        pc: RuntimePc::new(pc),
        samples,
    }
}

fn ranked(mut samples: Vec<PcSample>) -> Vec<(u64, u64)> {
    samples.sort_by(by_samples);

    samples
        .iter()
        .map(|sample| (sample.pc.raw(), sample.samples))
        .collect()
}

#[test]
fn the_busiest_counter_ranks_first() {
    assert_eq!(ranked(vec![sample(0x1000, 2), sample(0x2000, 40)]), vec![
        (0x2000, 40),
        (0x1000, 2)
    ]);
}

/// `pc_samples` promises a sorted sequence, and a sequence whose ties come out
/// in `HashMap` order is not one: the same table would yield a different list
/// on every run.
#[test]
fn a_tie_ranks_the_same_whichever_order_it_arrives_in() {
    assert_eq!(
        ranked(vec![
            sample(0x3000, 7),
            sample(0x1000, 7),
            sample(0x2000, 7)
        ]),
        vec![(0x1000, 7), (0x2000, 7), (0x3000, 7)]
    );
    assert_eq!(
        ranked(vec![
            sample(0x2000, 7),
            sample(0x3000, 7),
            sample(0x1000, 7)
        ]),
        vec![(0x1000, 7), (0x2000, 7), (0x3000, 7)]
    );
}
