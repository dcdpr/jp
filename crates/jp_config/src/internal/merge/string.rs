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
    if prev.discard_when_merged() {
        return Ok(Some(next));
    }

    let prev_value = match prev {
        PartialMergeableString::String(v) => Some(v),
        PartialMergeableString::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, PartialMergeableString::String(_));
    let (strategy, separator, next_value, discard_when_merged) = match next {
        PartialMergeableString::String(v) => {
            (Some(MergedStringStrategy::Replace), None, Some(v), None)
        }
        PartialMergeableString::Merged(v) => {
            (v.strategy, v.separator, v.value, v.discard_when_merged)
        }
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
            discard_when_merged,
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
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    discard_when_merged: None,
                    separator: Some(MergedStringSeparator::None),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Space),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Space),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Line),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Line),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Paragraph),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo\n\nbar".to_owned()),
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Paragraph),
                    discard_when_merged: None,
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
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("barfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("barfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Replace),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Space),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Space),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Line),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar\nfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Line),
                    discard_when_merged: None,
                }),
            },
            TestCase {
                prev: PartialMergeableString::String("foo".to_owned()),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Paragraph),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar\n\nfoo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::Paragraph),
                    discard_when_merged: None,
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
                    discard_when_merged: Some(true),
                }),
                next: PartialMergeableString::String("bar".to_owned()),
                expected: PartialMergeableString::String("bar".to_owned()),
            }),
            ("default does not merge", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: None,
                    separator: None,
                    discard_when_merged: Some(true),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: None,
                }),
            }),
            ("default stacking", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: None,
                    separator: None,
                    discard_when_merged: Some(true),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: Some(true),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: Some(true),
                }),
            }),
            ("next as default", TestCase {
                prev: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("bar".to_owned()),
                    strategy: None,
                    separator: None,
                    discard_when_merged: Some(false),
                }),
                next: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foo".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: Some(true),
                }),
                expected: PartialMergeableString::Merged(PartialMergedString {
                    value: Some("foobar".to_owned()),
                    strategy: Some(MergedStringStrategy::Prepend),
                    separator: Some(MergedStringSeparator::None),
                    discard_when_merged: Some(true),
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
