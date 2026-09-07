//! The delta law, checked per collection strategy.
//!
//! A delta earns its name by reproducing `next` when folded onto `prev`:
//!
//! ```text
//! fold(prev, delta(prev, next)) == next
//! ```
//!
//! A collection carries its own merge strategy, so it can always satisfy the
//! law: where appending cannot reach `next`, the delta says `replace` and
//! carries the whole value.
//! These tests hold each collection to that, across every way one resolved
//! snapshot can differ from another.

use schematic::PartialConfig as _;
use test_log::test;

use crate::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    conversation::tool::access::{PartialAccessConfig, PartialEnvRuleConfig},
    delta::PartialConfigDelta as _,
    types::vec::{MergeableVec, MergedVec, MergedVecStrategy},
};

/// One environment-variable rule, named and granting read.
fn rule(name: &str) -> PartialEnvRuleConfig {
    PartialEnvRuleConfig {
        name: Some(name.to_owned()),
        read: Some(true),
    }
}

/// An access block whose `env` rules are a resolved snapshot.
///
/// `ToPartial` stamps `replace` onto a resolved list, so this is the shape both
/// sides of a delta actually arrive in.
fn snapshot(names: &[&str]) -> PartialAccessConfig {
    PartialAccessConfig {
        fs: MergeableVec::default(),
        env: MergeableVec::Merged(MergedVec {
            value: names.iter().map(|name| rule(name)).collect(),
            strategy: Some(MergedVecStrategy::Replace),
            dedup: None,
            discard_when_merged: false,
        }),
    }
}

/// The rule names of an access block, in order.
fn names(access: &PartialAccessConfig) -> Vec<String> {
    access
        .env
        .iter()
        .filter_map(|rule| rule.name.clone())
        .collect()
}

/// Assert that the delta between two snapshots folds back to `next`.
///
/// Order is part of the assertion: rules of equal specificity break toward the
/// one declared last, so a delta that reproduces the set but not the sequence
/// silently inverts precedence.
fn assert_law(before: &[&str], after: &[&str]) {
    let prev = snapshot(before);
    let next = snapshot(after);

    let delta = prev.delta(next.clone());

    let mut folded = prev;
    folded
        .merge(&(), delta)
        .expect("folding a delta cannot fail");

    assert_eq!(
        names(&folded),
        names(&next),
        "{before:?} -> {after:?} did not fold back to the new value"
    );
}

/// Fields whose clear is known not to survive a fold, and why.
///
/// `inherit` is never stored: [`PartialAppConfig`]'s delta zeroes it, because
/// it is interpreted while config is loaded and only its effect outlives that.
/// Clearing it is meaningless rather than unrecordable.
///
/// `conversation.compaction.rules` has built-in defaults carrying
/// `discard_when_merged`, so a resolved empty list and the resolved defaults
/// compare unequal while resolving alike.
/// A delta helper that judged them by their elements would write a
/// replace-with-empty for no change at all, which is how it was found: routing
/// it through [`delta_mergeable_vec`] turned 39 tests red with exactly that
/// noise.
///
/// `model.parameters.other` is the catch-all arm of its own key-value dispatch,
/// so `parameters.other` names a key *inside* the map rather than the map
/// itself, and clearing removes an entry that was never there.
/// Reaching the whole field needs a path vocabulary that can say "this map"
/// where the map is also the fallback.
///
/// `conversation.tools.*` addresses the tool defaults block, whose types have
/// no path-reporting delta yet.
/// Mechanical to add, and left for the pass that does the tool config as a
/// whole.
const CLEAR_NOT_RECORDED: &[&str] = &[
    "inherit",
    "conversation.compaction.rules",
    "assistant.model.parameters.other",
    "style.reasoning.summary_model.parameters.other",
    "conversation.inquiry.assistant.model.parameters.other",
    "conversation.title.generate.model.parameters.other",
    "conversation.tools.*.enable",
    "conversation.tools.*.enable.state",
    "conversation.tools.*.enable.allow_toggle",
    "conversation.tools.*.style.error.inline_results",
    "conversation.tools.*.style.error.results_file_link",
];

/// Set `path` to whichever of a few generic values it accepts.
///
/// A field has to hold something before clearing it proves anything, and there
/// is no generic way to ask a field for a value it would accept.
/// Trying a handful and keeping the first that parses reaches scalars and
/// collections alike; a path that accepts none of them stays as the fixture
/// left it.
fn populate(partial: &PartialAppConfig, path: &str) -> Option<PartialAppConfig> {
    for value in ["1", "true", "x", "[]", "{}", "off"] {
        let Ok(kv) = KvAssignment::try_from_cli(path, value) else {
            continue;
        };

        let mut candidate = partial.clone();
        if candidate.assign(kv).is_ok() {
            return Some(candidate);
        }
    }

    None
}

/// A config with as many fields set as the sweep can arrange.
///
/// A population that leaves the config unresolvable is dropped rather than
/// carried, so the fixture the sweep starts from is always valid.
/// The result is round-tripped through a resolved config, since that is the
/// shape both sides of a real delta arrive in.
fn populated_fixture() -> PartialAppConfig {
    let mut partial = crate::AppConfig::new_test().to_partial();

    for path in crate::AppConfig::fields() {
        let Some(candidate) = populate(&partial, &path) else {
            continue;
        };

        if crate::util::build(candidate.clone()).is_ok() {
            partial = candidate;
        }
    }

    crate::util::build(partial)
        .expect("the populated fixture resolves")
        .to_partial()
}

/// A field that went away reports its path, since merging cannot say it.
#[test]
fn a_cleared_scalar_reports_its_path() {
    let mut prev = PartialAppConfig::empty();
    prev.assistant.name = Some("Bot".to_owned());

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(PartialAppConfig::empty(), "", &mut unsets);

    assert_eq!(unsets, ["assistant.name"]);
    assert_eq!(
        delta.assistant.name, None,
        "the value is not carried; the clear is the whole change"
    );
}

/// Every field, asked whether clearing it survives a fold.
///
/// Shaped like the producer: a `--cfg foo=null` clears the field from the
/// partial, the invocation resolves it, and the delta is taken between two
/// resolved configs.
/// A field with a `#[setting(default)]` therefore comes back holding that
/// default rather than arriving cleared, and only a field whose resolved type
/// is `Option<T>` reaches the delta as an absence.
///
/// The law is checked on the resolved configs, since that is what a later turn
/// runs with.
///
/// Paths the fixture leaves unset cannot change when cleared, so they prove
/// nothing; the count is reported so the test says how much it actually
/// covered.
#[test]
fn clearing_any_field_survives_a_fold() {
    let prev = populated_fixture();

    let mut vacuous = Vec::new();
    let mut lost = Vec::new();

    for path in crate::AppConfig::fields() {
        let mut next = prev.clone();
        if next.unset(&path).is_err() {
            continue;
        }

        if next == prev {
            vacuous.push(path);
            continue;
        }

        // A clear that leaves the config invalid is not a case the producer has
        // to reproduce; the invocation that typed it fails instead.
        let Ok(expected) = crate::util::build(next) else {
            continue;
        };

        // The producer diffs two *resolved* configs, so `next` arrives through
        // this round trip. A field with a default comes back holding it, which
        // is why only a field whose resolved type is optional can arrive
        // cleared.
        let next = expected.to_partial();

        let mut unsets = Vec::new();
        let delta = prev.delta_with_unsets(next, "", &mut unsets);

        let mut folded = prev.clone();
        for cleared in &unsets {
            folded.unset(cleared).expect("a reported path is a field");
        }
        folded.merge(&(), delta).expect("folding cannot fail");

        if crate::util::build(folded).ok().as_ref() != Some(&expected)
            && !CLEAR_NOT_RECORDED.contains(&path.as_str())
        {
            lost.push(path);
        }
    }

    assert!(
        lost.is_empty(),
        "clearing these fields does not survive a fold: {lost:#?}"
    );

    // Reported rather than asserted on: the fixture is what it is, and a path it
    // leaves unset cannot change when cleared. Shrinking this list is how the
    // sweep's reach grows.
    eprintln!(
        "{} of {} paths were already unset in the fixture and proved nothing",
        vacuous.len(),
        crate::AppConfig::fields().len(),
    );
}

#[test]
fn law_holds_for_an_unchanged_list() {
    assert_law(&["A"], &["A"]);
}

#[test]
fn law_holds_for_an_appended_rule() {
    assert_law(&["A"], &["A", "B"]);
}

#[test]
fn law_holds_for_a_prepended_rule() {
    assert_law(&["A"], &["B", "A"]);
}

#[test]
fn law_holds_for_a_rule_inserted_in_the_middle() {
    assert_law(&["A", "C"], &["A", "B", "C"]);
}

#[test]
fn law_holds_for_a_removed_rule() {
    assert_law(&["A", "B"], &["A"]);
}

#[test]
fn law_holds_for_a_reordered_list() {
    assert_law(&["A", "B"], &["B", "A"]);
}

#[test]
fn law_holds_for_a_repeated_rule() {
    assert_law(&["A", "B"], &["A", "B", "A"]);
}

#[test]
fn law_holds_for_an_append_onto_a_list_that_already_repeats() {
    assert_law(&["A", "B", "A"], &["A", "B", "A", "C"]);
}

#[test]
fn law_holds_for_a_wholly_replaced_list() {
    assert_law(&["A"], &["B"]);
}

#[test]
fn law_holds_for_a_cleared_list() {
    assert_law(&["A"], &[]);
}

#[test]
fn law_holds_for_a_first_rule() {
    assert_law(&[], &["A"]);
}
