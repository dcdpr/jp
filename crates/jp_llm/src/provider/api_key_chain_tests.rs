use std::collections::BTreeMap;

use super::*;

/// A variable the environment always holds, so no test has to set one and race
/// the others.
fn set() -> &'static str {
    if cfg!(windows) { "USERNAME" } else { "USER" }
}

/// A second always-present variable, for a chain that needs two.
fn also_set() -> &'static str {
    if cfg!(windows) { "USERPROFILE" } else { "HOME" }
}

const UNSET: &str = "JP_TEST_CHAIN_KEY_UNSET";
const ALSO_UNSET: &str = "JP_TEST_CHAIN_KEY_ALSO_UNSET";

// Each test that mutates the environment uses its own variable, so the tests
// stay independent however the runner schedules them.
const BLANK: &str = "JP_TEST_CHAIN_KEY_BLANK";
const WHITESPACE: &str = "JP_TEST_CHAIN_KEY_WHITESPACE";
const VERBATIM: &str = "JP_TEST_CHAIN_KEY_VERBATIM";
const BLANK_IN_CHAIN: &str = "JP_TEST_CHAIN_KEY_BLANK_IN_CHAIN";

fn many(pairs: &[(&str, &str)]) -> ApiKeyEnv {
    ApiKeyEnv::Many(
        pairs
            .iter()
            .map(|(name, variable)| ((*name).to_owned(), (*variable).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn chain(entries: &[&str]) -> Vec<AuthEntry> {
    entries.iter().map(|e| e.parse().unwrap()).collect()
}

/// A variable exported with no value is not a credential.
#[test]
fn test_a_blank_variable_holds_no_key() {
    // SAFETY: single-threaded test, and the variable is read back immediately.
    unsafe {
        std::env::set_var(BLANK, "");
        std::env::set_var(WHITESPACE, "   \n");
    }

    assert_eq!(read_key(BLANK), None, "an empty value is not a key");
    assert_eq!(
        read_key(WHITESPACE),
        None,
        "a whitespace-only value is not a key"
    );
    assert_eq!(read_key(UNSET), None);

    unsafe {
        std::env::remove_var(BLANK);
        std::env::remove_var(WHITESPACE);
    }
}

/// A real key is passed through byte-exact.
#[test]
fn test_a_key_is_read_verbatim() {
    // SAFETY: single-threaded test, and the variable is read back immediately.
    unsafe { std::env::set_var(VERBATIM, " sk-1\n") }

    assert_eq!(read_key(VERBATIM).as_deref(), Some(" sk-1\n"));

    unsafe { std::env::remove_var(VERBATIM) }
}

/// A blank variable falls through as if absent.
#[test]
fn test_a_blank_key_falls_through_to_the_next_entry() {
    // SAFETY: single-threaded test, and the variable is read back immediately.
    unsafe { std::env::set_var(BLANK_IN_CHAIN, "") }

    let expected = std::env::var(set()).unwrap();
    let keys = many(&[("primary", BLANK_IN_CHAIN), ("fallback", set())]);

    let (key, selected) = resolve(
        "cerebras",
        &chain(&["api_key:primary", "api_key:fallback"]),
        &keys,
    )
    .unwrap();

    assert_eq!(key, expected);
    assert_eq!(selected, AuthEntry::ApiKey(Some("fallback".to_owned())));

    unsafe { std::env::remove_var(BLANK_IN_CHAIN) }
}

/// The default chain, behaving as a bare environment read always did.
#[test]
fn test_single_entry_resolves_the_sole_key() {
    let expected = std::env::var(set()).unwrap();

    let (key, selected) = resolve("cerebras", &chain(&["api_key"]), &set().into()).unwrap();

    assert_eq!(key, expected);
    assert_eq!(selected, AuthEntry::ApiKey(None));
}

/// A single-entry chain fails as a missing variable, not as an exhausted chain.
#[test]
fn test_single_entry_reports_the_missing_variable() {
    let error = resolve("cerebras", &chain(&["api_key"]), &UNSET.into()).unwrap_err();

    assert!(
        matches!(&error, ChainError::MissingEnv(name) if name == UNSET),
        "unexpected error: {error}"
    );
}

#[test]
fn test_named_key_is_selected_by_name() {
    let expected = std::env::var(also_set()).unwrap();
    let keys = many(&[("personal", UNSET), ("work", also_set())]);

    let (key, selected) = resolve("cerebras", &chain(&["api_key:work"]), &keys).unwrap();

    assert_eq!(key, expected);
    assert_eq!(selected, AuthEntry::ApiKey(Some("work".to_owned())));
}

/// With no store to consult, a bare name can only be one of the keys.
#[test]
fn test_a_bare_name_selects_a_key() {
    let expected = std::env::var(also_set()).unwrap();
    let keys = many(&[("work", also_set())]);

    let (key, selected) = resolve("cerebras", &chain(&["work"]), &keys).unwrap();

    assert_eq!(key, expected);
    assert_eq!(selected, AuthEntry::ApiKey(Some("work".to_owned())));
}

/// The point of a chain: an unset key falls through to the next one.
#[test]
fn test_an_unset_key_falls_through_to_the_next_entry() {
    let expected = std::env::var(set()).unwrap();
    let keys = many(&[("primary", UNSET), ("fallback", set())]);

    let (key, selected) = resolve(
        "cerebras",
        &chain(&["api_key:primary", "api_key:fallback"]),
        &keys,
    )
    .unwrap();

    assert_eq!(key, expected);
    assert_eq!(selected, AuthEntry::ApiKey(Some("fallback".to_owned())));
}

/// A chain that resolves nothing names every entry it skipped, so the user can
/// see which variables to set rather than guessing.
#[test]
fn test_exhausted_chain_lists_every_skipped_entry() {
    let keys = many(&[("primary", UNSET), ("fallback", ALSO_UNSET)]);

    let error = resolve(
        "cerebras",
        &chain(&["api_key:primary", "api_key:fallback"]),
        &keys,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("api_key:primary"), "{error}");
    assert!(error.contains("api_key:fallback"), "{error}");
    assert!(error.contains(UNSET), "{error}");
}

/// A provider with no subscription says so, rather than skipping the entry and
/// reporting an exhausted chain that never mentions the real problem.
#[test]
fn test_a_subscription_entry_is_rejected_by_name() {
    let error = resolve("cerebras", &chain(&["subscription"]), &set().into())
        .unwrap_err()
        .to_string();

    assert!(error.contains("subscription"), "{error}");
    assert!(error.contains("cerebras"), "{error}");
    assert!(error.contains("api_key"), "{error}");
}

/// A name no key answers to is a config mistake, not a credential to skip:
/// falling through would bill a different key than the one asked for.
#[test]
fn test_an_unknown_name_is_an_error_even_with_later_entries() {
    let keys = many(&[("work", set())]);

    let error = resolve("cerebras", &chain(&["api_key:missing", "work"]), &keys)
        .unwrap_err()
        .to_string();

    assert!(error.contains("missing"), "{error}");
}
