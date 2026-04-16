use serde_json::json;
use url::Url;

use super::*;

fn opts(value: &serde_json::Value) -> WebFetchOptions {
    let map = value.as_object().cloned().unwrap_or_default();
    WebFetchOptions::parse(&map).expect("valid options")
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn empty_options_default_to_auto() {
    let o = opts(&json!({}));
    assert_eq!(
        o.pick_strategy(&url("https://example.com/x")),
        Strategy::Auto
    );
}

#[test]
fn strategy_override_applies_globally() {
    let o = opts(&json!({ "strategy": "html" }));
    assert_eq!(
        o.pick_strategy(&url("https://example.com/x")),
        Strategy::Html
    );
}

#[test]
fn exact_domain_match_wins_over_default() {
    let o = opts(&json!({
        "strategy": "html",
        "domains": { "docs.anthropic.com": "markdown" }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://docs.anthropic.com/api/rate-limits")),
        Strategy::Markdown
    );
    assert_eq!(
        o.pick_strategy(&url("https://example.com/x")),
        Strategy::Html
    );
}

#[test]
fn suffix_wildcard_matches_subdomains_and_apex() {
    let o = opts(&json!({
        "domains": { "*.mintlify.app": "markdown" }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://foo.mintlify.app/docs")),
        Strategy::Markdown
    );
    assert_eq!(
        o.pick_strategy(&url("https://deep.sub.mintlify.app/docs")),
        Strategy::Markdown
    );
    assert_eq!(
        o.pick_strategy(&url("https://mintlify.app/docs")),
        Strategy::Markdown
    );
    // Not a real subdomain — must not match.
    assert_eq!(
        o.pick_strategy(&url("https://evil-mintlify.app/docs")),
        Strategy::Auto
    );
}

#[test]
fn exact_wins_over_wildcard() {
    let o = opts(&json!({
        "domains": {
            "*.mintlify.app": "markdown",
            "docs.mintlify.app": "html"
        }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://docs.mintlify.app/x")),
        Strategy::Html
    );
    assert_eq!(
        o.pick_strategy(&url("https://other.mintlify.app/x")),
        Strategy::Markdown
    );
}

#[test]
fn longest_wildcard_wins_over_shorter() {
    let o = opts(&json!({
        "domains": {
            "*.app": "html",
            "*.mintlify.app": "markdown"
        }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://docs.mintlify.app/x")),
        Strategy::Markdown
    );
    assert_eq!(
        o.pick_strategy(&url("https://docs.other.app/x")),
        Strategy::Html
    );
}

#[test]
fn unknown_strategy_errors() {
    let map = json!({ "strategy": "nonsense" })
        .as_object()
        .cloned()
        .unwrap();
    assert!(WebFetchOptions::parse(&map).is_err());
}

#[test]
fn unknown_field_errors() {
    let map = json!({ "typo": true }).as_object().cloned().unwrap();
    assert!(WebFetchOptions::parse(&map).is_err());
}
