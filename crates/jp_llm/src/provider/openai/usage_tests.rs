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

/// The listing keeps redeemed credits alongside available ones, so what is left
/// comes from the count the backend reports, not the list's length.
#[test]
fn the_available_count_ignores_redeemed_credits() {
    let body = r#"{
        "credits": [
            { "id": "c1", "reset_type": "codex_rate_limits", "status": "redeemed",
              "granted_at": "2026-09-01T00:00:00Z", "expires_at": null },
            { "id": "c2", "reset_type": "codex_rate_limits", "status": "available",
              "granted_at": "2026-09-02T00:00:00Z", "expires_at": null }
        ],
        "available_count": 1
    }"#;

    assert_eq!(
        parse_credits(body),
        Some(ResetCredits { available_count: 1 })
    );
}

/// A redemption names no credit, so a redeemed one earlier in the listing can
/// never be the one asked for.
#[test]
fn a_redemption_leaves_the_choice_of_credit_to_the_backend() {
    assert_eq!(
        consume_body("redeem-1"),
        serde_json::json!({ "redeem_request_id": "redeem-1" })
    );
}

#[test]
fn a_reset_reopens_the_window() {
    for body in [
        r#"{ "code": "reset", "windows_reset": 1 }"#,
        r#"{ "code": "already_redeemed", "windows_reset": 1 }"#,
    ] {
        assert_eq!(redemption_outcome(body), Redemption::Reopened, "{body}");
    }
}

/// The endpoint answers `200` for a redemption that did nothing.
#[test]
fn a_200_that_redeemed_nothing_is_a_refusal() {
    for body in [
        r#"{ "code": "no_credit", "windows_reset": 0 }"#,
        r#"{ "code": "nothing_to_reset", "windows_reset": 0 }"#,
    ] {
        assert_eq!(redemption_outcome(body), Redemption::Refused, "{body}");
    }
}

/// A body that names no known outcome confirms nothing, so it is not read as a
/// refusal the caller would record a cooldown over.
#[test]
fn an_unreadable_outcome_is_unknown() {
    for body in [r#"{ "code": "something_new" }"#, "{}", "not json"] {
        assert_eq!(redemption_outcome(body), Redemption::Unknown, "{body}");
    }
}

/// A trailing slash is not a path segment.
#[test]
fn a_trailing_slash_is_trimmed() {
    assert_eq!(
        root("https://chatgpt.com/backend-api/codex/"),
        "https://chatgpt.com/backend-api"
    );
}
