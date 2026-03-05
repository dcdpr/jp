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

/// Regression test: a finalized `MergeableString` round-tripped through
/// `to_partial()` must not re-apply the append/prepend merge strategy
/// when merged back into a partial that already has the same value.
///
/// This simulates the flow in `apply_conversation_config`:
///
/// 1. Config files produce a partial with `strategy: Append`.
/// 2. The conversation stream's finalized config is converted to a
///    partial via `to_partial()`.
/// 3. That partial is merged on top of the config file partial.
/// 4. The value must NOT be doubled.
#[test]
fn test_finalized_round_trip_does_not_double_append() {
    use schematic::Config as _;

    use crate::{partial::ToPartial as _, types::string::MergeableString};

    // Step 1: Config file provides system_prompt with append strategy.
    let config_file_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are JP.".to_owned()),
        strategy: Some(MergedStringStrategy::Append),
        separator: Some(MergedStringSeparator::Space),
        discard_when_merged: None,
    });

    // Step 2: Simulate a finalized config that was previously built from
    // the same config file partial (merged over the default, which has
    // `discard_when_merged: true`).
    let default_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
    });

    let merged = string_with_strategy(default_partial, config_file_partial.clone(), &())
        .unwrap()
        .unwrap();

    // The default is discarded, so the merged value is just the config
    // file value.
    assert_eq!(merged.as_ref(), "You are JP.");

    // Step 3: Finalize and round-trip through `to_partial()`, simulating
    // `stream.config().map(|c| c.to_partial())`.
    let finalized = MergeableString::from_partial(merged, vec![]).unwrap();
    let round_tripped = finalized.to_partial();

    // Step 4: Merge the round-tripped partial on top of the config file
    // partial (this is what `apply_conversation_config` does via
    // `load_partial`).
    let result = string_with_strategy(config_file_partial, round_tripped, &())
        .unwrap()
        .unwrap();

    // BUG: Without the fix, the value is "You are JP. You are JP."
    // because the append strategy is re-applied.
    assert_eq!(
        result.as_ref(),
        "You are JP.",
        "finalized config round-tripped via to_partial() should not re-apply the append strategy"
    );
}

/// Same as above, but for prepend strategy.
#[test]
fn test_finalized_round_trip_does_not_double_prepend() {
    use schematic::Config as _;

    use crate::{partial::ToPartial as _, types::string::MergeableString};

    let config_file_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are JP.".to_owned()),
        strategy: Some(MergedStringStrategy::Prepend),
        separator: Some(MergedStringSeparator::Space),
        discard_when_merged: None,
    });

    let default_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
    });

    let merged = string_with_strategy(default_partial, config_file_partial.clone(), &())
        .unwrap()
        .unwrap();

    assert_eq!(merged.as_ref(), "You are JP.");

    let finalized = MergeableString::from_partial(merged, vec![]).unwrap();
    let round_tripped = finalized.to_partial();

    let result = string_with_strategy(config_file_partial, round_tripped, &())
        .unwrap()
        .unwrap();

    assert_eq!(
        result.as_ref(),
        "You are JP.",
        "finalized config round-tripped via to_partial() should not re-apply the prepend strategy"
    );
}
