use std::str::FromStr as _;

use super::*;

fn parse(s: &str) -> AttachmentUrlOrPath {
    AttachmentUrlOrPath::from_str(s).expect("infallible")
}

fn assert_url(s: &str, expected: &str) {
    match parse(s) {
        AttachmentUrlOrPath::Url(u) => assert_eq!(u.as_str(), expected, "input: {s}"),
        AttachmentUrlOrPath::Path(p) => panic!("expected URL for {s:?}, got path: {p}"),
    }
}

fn assert_path(s: &str, expected: &str) {
    match parse(s) {
        AttachmentUrlOrPath::Path(p) => assert_eq!(p.as_str(), expected, "input: {s}"),
        AttachmentUrlOrPath::Url(u) => panic!("expected path for {s:?}, got URL: {u}"),
    }
}

#[test]
fn already_a_url_passes_through() {
    assert_url("https://example.com/x", "https://example.com/x");
    assert_url("file:///etc/hosts", "file:///etc/hosts");
    assert_url("jp://jp-c1234?select=a:-1", "jp://jp-c1234?select=a:-1");
}

#[test]
fn bare_jp_id_rewrites_to_jp_scheme() {
    assert_url("jp-c1234", "jp://jp-c1234");
}

#[test]
fn shorthand_value_only_becomes_implicit_select() {
    // The bare `a:-1` after `?` is the value of `select=` by default.
    assert_url("jp-c1234?a:-1", "jp://jp-c1234?select=a:-1");
    assert_url("jp-c1234?u,a:-3..", "jp://jp-c1234?select=u,a:-3..");
}

#[test]
fn shorthand_explicit_select_passes_through() {
    assert_url("jp-c1234?select=a:-1", "jp://jp-c1234?select=a:-1");
}

#[test]
fn shorthand_raw_flag_passes_through() {
    assert_url("jp-c1234?raw", "jp://jp-c1234?raw");
    assert_url("jp-c1234?raw=all", "jp://jp-c1234?raw=all");
}

#[test]
fn shorthand_combines_select_and_raw() {
    assert_url(
        "jp-c1234?select=a:-1&raw=all",
        "jp://jp-c1234?select=a:-1&raw=all",
    );
    assert_url(
        "jp-c1234?raw&select=u,a:-1",
        "jp://jp-c1234?raw&select=u,a:-1",
    );
}

#[test]
fn shorthand_empty_query_drops_separator() {
    assert_url("jp-c1234?", "jp://jp-c1234");
}

#[test]
fn paths_with_dots_are_not_jp_ids() {
    // Files that happen to start with `jp-` but contain non-id characters
    // (e.g. `.`) must be left alone as paths.
    assert_path("jp-readme.md", "jp-readme.md");
    assert_path("./jp-c1234", "./jp-c1234");
}

#[test]
fn ordinary_file_paths_round_trip() {
    assert_path("docs/rfd/041-foo.md", "docs/rfd/041-foo.md");
    assert_path("foo.txt", "foo.txt");
    assert_path("!docs/rfd/foo.md", "!docs/rfd/foo.md");
}

#[test]
fn starts_with_known_param_matches_known_names() {
    assert!(starts_with_known_param("select"));
    assert!(starts_with_known_param("select=a"));
    assert!(starts_with_known_param("select=a&raw"));
    assert!(starts_with_known_param("raw"));
    assert!(starts_with_known_param("raw=all"));
    assert!(!starts_with_known_param("a:-1"));
    assert!(!starts_with_known_param("selected=foo"));
}
