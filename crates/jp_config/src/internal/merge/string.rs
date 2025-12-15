//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::MergeResult;

use crate::types::string::{MergedStringStrategy, PartialMergeableString, PartialMergedString};

/// Merge two `PartialMergeableString ` values.
pub fn string_with_strategy(
    prev: PartialMergeableString,
    next: PartialMergeableString,
    _context: &(),
) -> MergeResult<PartialMergeableString> {
    let prev_value = match prev {
        PartialMergeableString::String(v) => Some(v),
        PartialMergeableString::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, PartialMergeableString::String(_));
    let (strategy, next_value) = match next {
        PartialMergeableString::String(v) => (Some(MergedStringStrategy::Replace), Some(v)),
        PartialMergeableString::Merged(v) => (v.strategy, v.value),
    };

    let separator = match strategy {
        Some(MergedStringStrategy::AppendSpace) => " ",
        Some(MergedStringStrategy::AppendLine) => "\n",
        Some(MergedStringStrategy::AppendParagraph) => "\n\n",
        _ => "",
    };

    let value = match (prev_value, next_value) {
        (_, n) if strategy == Some(MergedStringStrategy::Replace) => n,
        (Some(p), Some(n)) => Some(format!("{p}{separator}{n}")),
        (Some(p), None) => Some(p),
        (None, Some(n)) => Some(n),
        _ => None,
    };

    Ok(Some(if next_is_replace {
        PartialMergeableString::String(value.unwrap_or_default())
    } else {
        PartialMergeableString::Merged(PartialMergedString { value, strategy })
    }))
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn test_string_with_strategy() {
        struct TestCase {
            prev: PartialMergeableString,
            next: PartialMergeableString,
            expected: PartialMergeableString,
        }

        let cases = vec![
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendSpace),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo bar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendSpace),
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendLine),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendLine),
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendParagraph),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\n\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::AppendParagraph),
                }),
            },
        ];

        for TestCase {
            prev,
            next,
            expected,
        } in cases
        {
            let result = string_with_strategy(prev, next, &());
            assert_eq!(result.unwrap(), Some(expected));
        }
    }
}
