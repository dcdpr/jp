use serde_json::json;
use url::Url;

use super::*;

fn opts(value: &serde_json::Value) -> WebFetchOptions {
    let map = value.as_object().cloned().unwrap_or_default();
    WebFetchOptions::parse(&map)
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
fn strategy_parse_accepts_aliases() {
    assert_eq!(Strategy::parse("md"), Some(Strategy::Markdown));
    assert_eq!(Strategy::parse("MARKDOWN"), Some(Strategy::Markdown));
    assert_eq!(Strategy::parse(" Html "), Some(Strategy::Html));
    assert_eq!(Strategy::parse("nonsense"), None);
}

#[test]
fn unknown_strategy_falls_back_to_default() {
    let o = opts(&json!({ "strategy": "nonsense" }));
    assert_eq!(
        o.pick_strategy(&url("https://example.com/x")),
        Strategy::Auto
    );
}

#[test]
fn exact_domain_match_wins() {
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
fn suffix_wildcard_matches_subdomains_and_root() {
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
    assert_eq!(
        o.pick_strategy(&url("https://evil-mintlify.app/docs")),
        Strategy::Auto
    );
}

#[test]
fn host_matching_is_case_insensitive() {
    let o = opts(&json!({
        "domains": { "Docs.Anthropic.Com": "markdown" }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://DOCS.anthropic.com/x")),
        Strategy::Markdown
    );
}

#[test]
fn malformed_domain_value_is_ignored() {
    let o = opts(&json!({
        "domains": {
            "docs.anthropic.com": "markdown",
            "bad.com": 42,
            "other.com": "nope"
        }
    }));
    assert_eq!(
        o.pick_strategy(&url("https://docs.anthropic.com/x")),
        Strategy::Markdown
    );
    // Non-string and unknown strategy entries silently drop out.
    assert_eq!(o.pick_strategy(&url("https://bad.com/x")), Strategy::Auto);
    assert_eq!(o.pick_strategy(&url("https://other.com/x")), Strategy::Auto);
}
