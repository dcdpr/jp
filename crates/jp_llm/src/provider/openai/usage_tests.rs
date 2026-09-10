use super::*;

/// The account endpoints are siblings of the responses host, not children, so
/// the trailing segment is dropped rather than appended to.
#[test]
fn account_endpoints_hang_off_the_backend_root() {
    assert_eq!(
        root("https://chatgpt.com/backend-api/codex"),
        "https://chatgpt.com/backend-api"
    );
}

/// A recording server is addressed as a bare host, which has no segment to
/// drop.
#[test]
fn a_bare_host_is_its_own_root() {
    assert_eq!(root("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
}

/// A trailing slash is not a path segment.
#[test]
fn a_trailing_slash_is_trimmed() {
    assert_eq!(
        root("https://chatgpt.com/backend-api/codex/"),
        "https://chatgpt.com/backend-api"
    );
}
