//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::{MergeResult, Schematic};
use serde::{de::DeserializeOwned, Serialize};

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
    let mut prev_value = match prev {
        MergeableVec::Vec(v) => v,
        MergeableVec::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, MergeableVec::Vec(_));
    let (strategy, mut next_value) = match next {
        MergeableVec::Vec(v) => (MergedVecStrategy::Replace, v),
        MergeableVec::Merged(v) => (v.strategy, v.value),
    };

    let value = match strategy {
        MergedVecStrategy::Append => {
            prev_value.append(&mut next_value);
            prev_value
        }
        MergedVecStrategy::Replace => next_value,
    };

    Ok(Some(if next_is_replace {
        MergeableVec::Vec(value)
    } else {
        MergeableVec::Merged(MergedVec { value, strategy })
    }))
}

#[cfg(test)]
mod tests {
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
                expected: MergeableVec::Vec(vec![4, 5, 6]),
            },
            TestCase {
                prev: MergeableVec::Vec(vec![1, 2, 3]),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Append,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3, 4, 5, 6],
                    strategy: MergedVecStrategy::Append,
                }),
            },
            TestCase {
                prev: MergeableVec::Vec(vec![1, 2, 3]),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: MergedVecStrategy::Append,
                }),
                next: MergeableVec::Vec(vec![4, 5, 6]),
                expected: MergeableVec::Vec(vec![4, 5, 6]),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: MergedVecStrategy::Append,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: MergedVecStrategy::Append,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Append,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3, 4, 5, 6],
                    strategy: MergedVecStrategy::Append,
                }),
            },
            TestCase {
                prev: MergeableVec::Merged(MergedVec {
                    value: vec![1, 2, 3],
                    strategy: MergedVecStrategy::Append,
                }),
                next: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
                }),
                expected: MergeableVec::Merged(MergedVec {
                    value: vec![4, 5, 6],
                    strategy: MergedVecStrategy::Replace,
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
}
