use test_log::test;

use super::*;
use crate::types::vec::MergedVecStrategy;

#[test]
fn test_vec_with_strategy() {
    struct TestCase {
        prev: MergeableVec<usize>,
        next: MergeableVec<usize>,
        expected: MergeableVec<usize>,
    }

    let cases = vec![
        TestCase {
            prev: MergeableVec::Vec(vec![1, 2, 3]),
            next: MergeableVec::Vec(vec![4, 5, 6]),
            expected: MergeableVec::Vec(vec![1, 2, 3, 4, 5, 6]),
        },
        TestCase {
            prev: MergeableVec::Vec(vec![1, 2, 3]),
            next: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3, 4, 5, 6],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
        },
        TestCase {
            prev: MergeableVec::Vec(vec![1, 2, 3]),
            next: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
        },
        TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            next: MergeableVec::Vec(vec![4, 5, 6]),
            expected: MergeableVec::Vec(vec![1, 2, 3, 4, 5, 6]),
        },
        TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
        },
        TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3, 4, 5, 6],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
        },
        TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1, 2, 3],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![4, 5, 6],
                strategy: Some(MergedVecStrategy::Replace),
                dedup: None,
                discard_when_merged: false,
            }),
        },
    ];

    for TestCase {
        prev,
        next,
        expected,
    } in cases
    {
        let result = vec_with_strategy(prev, next, &());
        assert_eq!(result.unwrap(), Some(expected));
    }
}

#[test]
fn test_default_vec() {
    struct TestCase {
        prev: MergeableVec<usize>,
        next: MergeableVec<usize>,
        expected: MergeableVec<usize>,
    }

    let cases = vec![
        ("default with next string", TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
            next: MergeableVec::Vec(vec![2]),
            expected: MergeableVec::Vec(vec![2]),
        }),
        ("default does not merge", TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
        }),
        ("default stacking", TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
        }),
        ("next as default", TestCase {
            prev: MergeableVec::Merged(MergedVec {
                value: vec![1],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: false,
            }),
            next: MergeableVec::Merged(MergedVec {
                value: vec![2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
            expected: MergeableVec::Merged(MergedVec {
                value: vec![1, 2],
                strategy: Some(MergedVecStrategy::Append),
                dedup: None,
                discard_when_merged: true,
            }),
        }),
    ];

    for (
        name,
        TestCase {
            prev,
            next,
            expected,
        },
    ) in cases
    {
        let result = vec_with_strategy(prev, next, &());
        assert_eq!(result.unwrap(), Some(expected), "test case: {name}");
    }
}

#[test]
fn test_dedup_removes_duplicates_on_append() {
    let prev = MergeableVec::Merged(MergedVec {
        value: vec![1, 2, 3],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: false,
    });
    let next = MergeableVec::Vec(vec![2, 3, 4]);

    let result = vec_with_strategy(prev, next, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 3, 4]);
    assert!(result.dedup(), "dedup flag should be sticky");
}

#[test]
fn test_discard_inherits_dedup_when_next_has_no_opinion() {
    // A discarded default's dedup flag inherits to next when next
    // has dedup: None ("inherit").
    let default = MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: true,
    });
    let config = MergeableVec::Vec(vec![1, 2, 1, 3]);

    let result = vec_with_strategy(default, config, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 3]);
    assert!(result.dedup());
}

#[test]
fn test_discard_does_not_inherit_dedup_when_next_opts_out() {
    // If next explicitly sets dedup: false, it overrides the default.
    let default = MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: true,
    });
    let config = MergeableVec::Merged(MergedVec {
        value: vec![1, 2, 1, 3],
        strategy: None,
        dedup: Some(false),
        discard_when_merged: false,
    });

    let result = vec_with_strategy(default, config, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 1, 3]);
    assert!(!result.dedup());
}

#[test]
fn test_dedup_sticky_across_non_discarded_merges() {
    // Once a real (non-discarded) config sets dedup, it sticks.
    let base = MergeableVec::Merged(MergedVec {
        value: vec![1, 2],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: false,
    });
    let overlay = MergeableVec::Vec(vec![2, 3]);

    let result = vec_with_strategy(base, overlay, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 3]);
    assert!(result.dedup());

    // Third merge — flag still sticky.
    let more = MergeableVec::Vec(vec![3, 4]);
    let result = vec_with_strategy(result, more, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 3, 4]);
    assert!(result.dedup());
}

#[test]
fn test_no_dedup_without_flag() {
    let prev = MergeableVec::Vec(vec![1, 2]);
    let next = MergeableVec::Vec(vec![2, 3]);

    let result = vec_with_strategy(prev, next, &()).unwrap().unwrap();
    assert_eq!(&*result, &[1, 2, 2, 3]);
}

#[test]
fn test_is_empty_vec_empty() {
    let v: MergeableVec<i32> = MergeableVec::Vec(vec![]);
    assert!(v.is_empty());
}

#[test]
fn test_is_empty_vec_non_empty() {
    let v = MergeableVec::Vec(vec![1]);
    assert!(!v.is_empty());
}

#[test]
fn test_is_empty_merged_with_dedup() {
    let v: MergeableVec<i32> = MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: false,
    });
    assert!(!v.is_empty(), "metadata-only Merged should not be empty");
}

#[test]
fn test_is_empty_merged_with_strategy() {
    let v: MergeableVec<i32> = MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    });
    assert!(!v.is_empty());
}

#[test]
fn test_is_empty_merged_no_metadata() {
    let v: MergeableVec<i32> = MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: None,
        dedup: None,
        discard_when_merged: false,
    });
    assert!(v.is_empty());
}
