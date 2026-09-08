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
