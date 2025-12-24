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
    // If prev is default, replace regardless of strategy.
    if prev.is_default() {
        return Ok(Some(next));
    }

    let prev_value = match prev {
        PartialMergeableString::String(v) => Some(v),
        PartialMergeableString::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, PartialMergeableString::String(_));
    let (strategy, separator, next_value, is_default) = match next {
        PartialMergeableString::String(v) => {
            (Some(MergedStringStrategy::Replace), None, Some(v), None)
        }
        PartialMergeableString::Merged(v) => (v.strategy, v.separator, v.value, v.is_default),
    };

    let sep = separator.as_ref().map_or("", |sep| sep.as_str());
    let value = match (prev_value, next_value) {
        (_, n) if strategy == Some(MergedStringStrategy::Replace) => n,
        (Some(p), Some(n)) if strategy == Some(MergedStringStrategy::Append) => {
            Some(format!("{p}{sep}{n}"))
        }
        (Some(p), Some(n)) if strategy == Some(MergedStringStrategy::Prepend) => {
            Some(format!("{n}{sep}{p}"))
        }
        (Some(p), None) => Some(p),
        (None, Some(n)) => Some(n),
        _ => None,
    };

    Ok(Some(if next_is_replace {
        PartialMergeableString::String(value.unwrap_or_default())
    } else {
        PartialMergeableString::Merged(PartialMergedString {
            value,
            strategy,
            separator,
            is_default,
        })
    }))
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;
    use crate::types::string::MergedStringSeparator;

    #[test]
    fn test_string_with_append_strategy() {
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
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    is_default: None,
                    separator: Some(MergedStringSeparator::None),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Space),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Space),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Line),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Line),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Paragraph),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\n\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Paragraph),
                    is_default: None,
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

    #[test]
    fn test_string_with_prepend_strategy() {
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
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("barfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("barfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Space),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Space),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Line),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar\nfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Line),
                    is_default: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Paragraph),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar\n\nfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Paragraph),
                    is_default: None,
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

    #[test]
    fn test_default_string() {
        struct TestCase {
            prev: PartialMergeableString,
            next: PartialMergeableString,
            expected: PartialMergeableString,
        }

        let cases = vec![
            ("default with next string", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: None,
                    separator: None,
                    is_default: Some(true),
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            }),
            ("default does not merge", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: None,
                    separator: None,
                    is_default: Some(true),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: None,
                }),
            }),
            ("default stacking", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: None,
                    separator: None,
                    is_default: Some(true),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: Some(true),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: Some(true),
                }),
            }),
            ("next as default", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: None,
                    separator: None,
                    is_default: Some(false),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: Some(true),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    is_default: Some(true),
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
            let result = string_with_strategy(prev, next, &());
            assert_eq!(result.unwrap(), Some(expected), "test case: {name}");
        }
    }
}
