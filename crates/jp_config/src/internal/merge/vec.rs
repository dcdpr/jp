//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::{MergeResult, Schematic};
use serde::{Serialize, de::DeserializeOwned};

use crate::types::vec::{MergeableVec, MergedVec, MergedVecStrategy};

/// Merge two `MergeableVec` values.
pub fn vec_with_strategy<T>(
    prev: MergeableVec<T>,
    next: MergeableVec<T>,
    _context: &(),
) -> MergeResult<MergeableVec<T>>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    // If prev is default, replace regardless of strategy.
    if prev.discard_when_merged() {
        return Ok(Some(next));
    }

    let mut prev_value = match prev {
        MergeableVec::Vec(v) => v,
        MergeableVec::Merged(v) => v.value,
    };

    let next_is_merged = matches!(next, MergeableVec::Merged(_));
    let (strategy, mut next_value, discard_when_merged) = match next {
        MergeableVec::Vec(v) => (None, v, false),
        MergeableVec::Merged(v) => (v.strategy, v.value, v.discard_when_merged),
    };

    let value = match strategy {
        None | Some(MergedVecStrategy::Append) => {
            prev_value.append(&mut next_value);
            prev_value
        }
        Some(MergedVecStrategy::Replace) => next_value,
    };

    Ok(Some(if next_is_merged {
        MergeableVec::Merged(MergedVec {
            value,
            strategy,
            discard_when_merged,
        })
    } else {
        MergeableVec::Vec(value)
    }))
}

#[cfg(test)]
mod tests {
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
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3, 4, 5, 6],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
            },
            TestCase {
                prev: MergeableVec::Vec(vec![1, 2, 3]),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
                    discard_when_merged: false,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                next: MergeableVec::Vec(vec![4, 5, 6]),
                expected: MergeableVec::Vec(vec![1, 2, 3, 4, 5, 6]),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
                    discard_when_merged: false,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3, 4, 5, 6],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: Some(MergedVecStrategy::Replace),
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
                    discard_when_merged: true,
                }),
                next: MergeableVec::Vec(vec![2]),
                expected: MergeableVec::Vec(vec![2]),
            }),
            ("default does not merge", TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: true,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![2],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![2],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
            }),
            ("default stacking", TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: true,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![2],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: true,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![2],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: true,
                }),
            }),
            ("next as default", TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: false,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![2],
                    strategy: Some(MergedVecStrategy::Append),
                    discard_when_merged: true,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2],
                    strategy: Some(MergedVecStrategy::Append),
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
}
