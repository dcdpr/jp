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
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foobar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
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
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foobar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                discard_when_merged: None,
                dedup: None,
                separator: Some(MergedStringSeparator::None),
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Space),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo bar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Space),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Line),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo\nbar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Line),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Paragraph),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo\n\nbar".to_owned()),
                strategy: Some(MergedStringStrategy::Append),
                separator: Some(MergedStringSeparator::Paragraph),
                discard_when_merged: None,
                dedup: None,
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
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("barfoo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
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
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("barfoo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Replace),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Space),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Space),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Line),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar\nfoo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Line),
                discard_when_merged: None,
                dedup: None,
            }),
        },
        TestCase {
            prev: PartialMergeableString::String("foo".to_owned()),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Paragraph),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar\n\nfoo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::Paragraph),
                discard_when_merged: None,
                dedup: None,
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
                dedup: None,
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
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: None,
                dedup: None,
            }),
        }),
        ("default stacking", TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: None,
                separator: None,
                discard_when_merged: Some(true),
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: Some(true),
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: Some(true),
                dedup: None,
            }),
        }),
        ("next as default", TestCase {
            prev: PartialMergeableString::Merged(PartialMergedString {
                value: Some("bar".to_owned()),
                strategy: None,
                separator: None,
                discard_when_merged: Some(false),
                dedup: None,
            }),
            next: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: Some(true),
                dedup: None,
            }),
            expected: PartialMergeableString::Merged(PartialMergedString {
                value: Some("foobar".to_owned()),
                strategy: Some(MergedStringStrategy::Prepend),
                separator: Some(MergedStringSeparator::None),
                discard_when_merged: Some(true),
                dedup: None,
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
/// `to_partial()` must not re-apply the append/prepend merge strategy when
/// merged back into a partial that already has the same value.
///
/// This simulates the flow in `apply_conversation_config`:
///
/// 1. Config files produce a partial with `strategy: Append`.
/// 2. The conversation stream's finalized config is converted to a partial via
///    `to_partial()`.
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
        dedup: None,
    });

    // Step 2: Simulate a finalized config that was previously built from
    // the same config file partial (merged over the default, which has
    // `discard_when_merged: true`).
    let default_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
        dedup: None,
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
        dedup: None,
    });

    let default_partial = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
        dedup: None,
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

/// Build an appending partial with a paragraph separator.
fn appending(value: &str, dedup: Option<bool>) -> PartialMergeableString {
    PartialMergeableString::Merged(PartialMergedString {
        value: Some(value.to_owned()),
        strategy: Some(MergedStringStrategy::Append),
        separator: Some(MergedStringSeparator::Paragraph),
        discard_when_merged: None,
        dedup,
    })
}

#[test]
fn test_append_skips_value_already_present_as_block() {
    // A persona file appends a knowledge block and its own prompt. Supplying
    // the same source again — a second `--cfg` on an existing conversation, or
    // an `extends` diamond — must leave the accumulated value untouched.
    let accumulated = PartialMergeableString::String("base\n\nknowledge\n\npersona".to_owned());

    let result = string_with_strategy(accumulated, appending("knowledge", None), &())
        .unwrap()
        .unwrap();
    assert_eq!(result.as_ref(), "base\n\nknowledge\n\npersona");

    let result = string_with_strategy(result, appending("persona", None), &())
        .unwrap()
        .unwrap();
    assert_eq!(result.as_ref(), "base\n\nknowledge\n\npersona");
}

#[test]
fn test_append_matches_first_and_last_block() {
    let result = string_with_strategy(
        PartialMergeableString::String("first\n\nlast".to_owned()),
        appending("first", None),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.as_ref(), "first\n\nlast");

    let result = string_with_strategy(result, appending("last", None), &())
        .unwrap()
        .unwrap();
    assert_eq!(result.as_ref(), "first\n\nlast");
}

#[test]
fn test_append_ignores_match_inside_a_block() {
    // "brief" occurs inside a block but is not a block of its own, so it is a
    // genuinely new contribution and must be appended.
    let result = string_with_strategy(
        PartialMergeableString::String("Be brief and clear.".to_owned()),
        appending("brief", None),
        &(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(result.as_ref(), "Be brief and clear.\n\nbrief");
}

#[test]
fn test_append_without_separator_only_skips_exact_match() {
    // With no separator there are no block boundaries to anchor a match on, so
    // only a whole-string match counts as already present.
    let no_separator = |value: &str| {
        PartialMergeableString::Merged(PartialMergedString {
            value: Some(value.to_owned()),
            strategy: Some(MergedStringStrategy::Append),
            separator: Some(MergedStringSeparator::None),
            discard_when_merged: None,
            dedup: None,
        })
    };

    let result = string_with_strategy(
        PartialMergeableString::String("foobar".to_owned()),
        no_separator("bar"),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.as_ref(), "foobarbar");

    let result = string_with_strategy(
        PartialMergeableString::String("foo".to_owned()),
        no_separator("foo"),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.as_ref(), "foo");
}

#[test]
fn test_append_duplicates_when_dedup_disabled() {
    let result = string_with_strategy(
        PartialMergeableString::String("knowledge".to_owned()),
        appending("knowledge", Some(false)),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.as_ref(), "knowledge\n\nknowledge");

    // The opt-out is sticky: a later append with no opinion also duplicates.
    let result = string_with_strategy(result, appending("knowledge", None), &())
        .unwrap()
        .unwrap();
    assert_eq!(result.as_ref(), "knowledge\n\nknowledge\n\nknowledge");
}

#[test]
fn test_dedup_opt_out_survives_a_plain_string_replacement() {
    // A plain string states a value, not an opinion on dedup. It must not reset
    // an explicit opt-out set earlier in the chain, or the next append silently
    // deduplicates under the default.
    let opted_out = PartialMergeableString::Merged(PartialMergedString {
        value: Some("old".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: None,
        dedup: Some(false),
    });

    let replaced = string_with_strategy(
        opted_out,
        PartialMergeableString::String("new".to_owned()),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(replaced.as_ref(), "new");

    let result = string_with_strategy(replaced, appending("new", None), &())
        .unwrap()
        .unwrap();

    assert_eq!(result.as_ref(), "new\n\nnew");
}

#[test]
fn test_dedup_opt_out_survives_a_discarded_default() {
    // The built-in default is `discard_when_merged`, which returns `next`
    // wholesale. An opt-out on either side has to come through that path.
    let default = PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
        dedup: Some(false),
    });

    let replaced = string_with_strategy(
        default,
        PartialMergeableString::String("persona".to_owned()),
        &(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(replaced.as_ref(), "persona");

    let result = string_with_strategy(replaced, appending("persona", None), &())
        .unwrap()
        .unwrap();

    assert_eq!(result.as_ref(), "persona\n\npersona");
}

#[test]
fn test_plain_replacement_without_an_opinion_stays_a_plain_string() {
    // Carrying the flag needs the `Merged` wrapper, but only when there is an
    // opinion to carry: an ordinary replacement keeps its shape.
    let result = string_with_strategy(
        PartialMergeableString::String("old".to_owned()),
        PartialMergeableString::String("new".to_owned()),
        &(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        result,
        PartialMergeableString::String("new".to_owned()),
        "a replacement with no dedup opinion should not gain a Merged wrapper"
    );
}

#[test]
fn test_unstated_strategy_appends() {
    // `MergedStringStrategy` defaults to `append`, so a config that sets a
    // value without naming a strategy appends it.
    let no_strategy = PartialMergeableString::Merged(PartialMergedString {
        value: Some("bar".to_owned()),
        strategy: None,
        separator: Some(MergedStringSeparator::Space),
        discard_when_merged: None,
        dedup: None,
    });

    let result = string_with_strategy(
        PartialMergeableString::String("foo".to_owned()),
        no_strategy,
        &(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(result.as_ref(), "foo bar");
}

#[test]
fn test_dedup_accepts_inherit() {
    let merged: PartialMergedString =
        serde_json::from_str(r#"{"value":"foo","dedup":"inherit"}"#).unwrap();
    assert_eq!(merged.dedup, None);

    let merged: PartialMergedString =
        serde_json::from_str(r#"{"value":"foo","dedup":false}"#).unwrap();
    assert_eq!(merged.dedup, Some(false));

    let merged: PartialMergedString = serde_json::from_str(r#"{"value":"foo"}"#).unwrap();
    assert_eq!(merged.dedup, None);
}

#[test]
fn test_dedup_assignment_accepts_every_documented_value() {
    use crate::assignment::{AssignKeyValue as _, KvAssignment};

    // Every input channel accepts the values the field documents, so a
    // `--cfg assistant.system_prompt.dedup=inherit` behaves like the same value
    // written in a config file.
    for (input, want) in [
        ("true", Some(true)),
        ("false", Some(false)),
        ("inherit", None),
    ] {
        let mut partial = PartialMergedString::default();
        let kv = KvAssignment::try_from_cli("dedup", input).unwrap();
        partial.assign(kv).unwrap();

        assert_eq!(partial.dedup, want, "input: {input}");
    }

    let mut partial = PartialMergedString::default();
    let kv = KvAssignment::try_from_cli("dedup", "maybe").unwrap();
    assert!(
        partial.assign(kv).is_err(),
        "an undocumented value should be rejected"
    );
}

#[test]
fn test_prepend_skips_value_already_present_as_block() {
    let prepending = PartialMergeableString::Merged(PartialMergedString {
        value: Some("persona".to_owned()),
        strategy: Some(MergedStringStrategy::Prepend),
        separator: Some(MergedStringSeparator::Paragraph),
        discard_when_merged: None,
        dedup: None,
    });

    let result = string_with_strategy(
        PartialMergeableString::String("persona\n\nbase".to_owned()),
        prepending,
        &(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(result.as_ref(), "persona\n\nbase");
}
