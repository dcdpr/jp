//! Internal merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::MergeResult;

use crate::types::string::{
    PartialStringWithMerge, PartialStringWithStrategy, StringMergeStrategy,
};

/// Merge two `StringWithMerge ` values.
pub fn string_with_strategy(
    prev: PartialStringWithMerge,
    next: PartialStringWithMerge,
    _context: &(),
) -> MergeResult<PartialStringWithMerge> {
    let prev_value = match prev {
        PartialStringWithMerge::String(v) => Some(v),
        PartialStringWithMerge::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, PartialStringWithMerge::String(_));
    let (strategy, next_value) = match next {
        PartialStringWithMerge::String(v) => (Some(StringMergeStrategy::Replace), Some(v)),
        PartialStringWithMerge::Merged(v) => (v.strategy, v.value),
    };

    let separator = match strategy {
        Some(StringMergeStrategy::AppendSpace) => " ",
        Some(StringMergeStrategy::AppendLine) => "\n",
        Some(StringMergeStrategy::AppendParagraph) => "\n\n",
        _ => "",
    };

    let value = match (prev_value, next_value) {
        (_, n) if strategy == Some(StringMergeStrategy::Replace) => n,
        (Some(p), Some(n)) => Some(format!("{p}{separator}{n}")),
        (Some(p), None) => Some(p),
        (None, Some(n)) => Some(n),
        _ => None,
    };

    Ok(Some(if next_is_replace {
        PartialStringWithMerge::String(value.unwrap_or_default())
    } else {
        PartialStringWithMerge::Merged(PartialStringWithStrategy { value, strategy })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_string_with_strategy() {
        struct TestCase {
            prev: PartialStringWithMerge,
            next: PartialStringWithMerge,
            expected: PartialStringWithMerge,
        }

        let cases = vec![
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::String("bar".to_owned()),
                expected: PartialStringWithMerge::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foobar".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                next: PartialStringWithMerge::String("bar".to_owned()),
                expected: PartialStringWithMerge::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foobar".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo".to_owned()),
                    strategy: Some(StringMergeStrategy::Append),
                }),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::Replace),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendSpace),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo bar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendSpace),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendLine),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo\nbar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendLine),
                }),
            },
            TestCase {
                prev: PartialStringWithMerge::String("foo".to_owned()),
                next: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("bar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendParagraph),
                }),
                expected: PartialStringWithMerge::Merged(PartialStringWithStrategy {
                    value: Some("foo\n\nbar".to_owned()),
                    strategy: Some(StringMergeStrategy::AppendParagraph),
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
