use datetime_literal::datetime;
use test_log::test;

use super::*;

const NOW: fn() -> DateTime<Utc> = || datetime!(2026-07-03 12:00:00 Z);

/// The verifier must survive a URL round-trip unencoded and carry enough
/// entropy to be unguessable; RFC 7636 puts the floor at 43 characters.
#[test]
fn test_pkce_verifier_is_url_safe_and_long_enough() {
    let pkce = Pkce::generate();

    assert!(pkce.verifier.len() >= 43, "{}", pkce.verifier);
    assert!(
        pkce.verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "{}",
        pkce.verifier
    );
}

#[test]
fn test_pkce_pairs_are_distinct() {
    let first = Pkce::generate();
    let second = Pkce::generate();

    assert_ne!(first.verifier, second.verifier);
    assert_ne!(first.challenge, second.challenge);
}

/// The challenge is the `S256` hash of the verifier: the server recomputes it
/// from the verifier at exchange time, so an incorrect derivation fails the
/// whole flow.
///
/// The vector is RFC 7636's appendix B example.
#[test]
fn test_challenge_is_the_s256_hash_of_the_verifier() {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let digest = Sha256::digest(verifier.as_bytes());

    assert_eq!(
        base64_url(&digest),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn test_authorize_url_carries_the_pkce_and_scope_parameters() {
    let url = authorize_url(
        "challenge-value",
        "state-value",
        "http://localhost:7654/callback",
    );

    assert!(
        url.starts_with("https://claude.com/cai/oauth/authorize?"),
        "{url}"
    );
    assert!(
        url.contains("client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        "{url}"
    );
    assert!(url.contains("response_type=code"), "{url}");
    assert!(url.contains("code_challenge=challenge-value"), "{url}");
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(url.contains("state=state-value"), "{url}");

    // The redirect and the space-separated scope list must be encoded, or the
    // consent page reads them as extra parameters.
    assert!(
        url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A7654%2Fcallback"),
        "{url}"
    );
    assert!(
        url.contains("scope=user%3Aprofile%20user%3Ainference"),
        "{url}"
    );
}

/// JP asks for less than Claude Code's own grant: no API-key creation, no
/// session, MCP-server, or file-upload capabilities.
#[test]
fn test_authorize_url_requests_only_the_reduced_scope_set() {
    let url = authorize_url("c", "s", MANUAL_REDIRECT_URL);

    assert!(!url.contains("org%3Acreate_api_key"), "{url}");
    assert!(!url.contains("user%3Asessions"), "{url}");
    assert!(!url.contains("user%3Amcp_servers"), "{url}");
    assert!(!url.contains("user%3Afile_upload"), "{url}");
}

#[test]
fn test_exchange_body_shape() {
    let body = exchange_body(
        "the-code",
        "the-state",
        "the-verifier",
        "http://localhost:1/callback",
    );

    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], "the-code");
    assert_eq!(body["state"], "the-state");
    assert_eq!(body["code_verifier"], "the-verifier");
    assert_eq!(body["redirect_uri"], "http://localhost:1/callback");
    assert_eq!(body["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
}

#[test]
fn test_refresh_body_shape() {
    let body = refresh_body("the-refresh-token");

    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "the-refresh-token");
    assert_eq!(body["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(body["scope"], "user:profile user:inference");
}

/// The endpoint reports a *relative* lifetime, so the absolute expiry depends
/// on when the response arrived.
#[test]
fn test_expiry_is_resolved_against_the_response_time() {
    let tokens = parse_tokens(
        r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#,
        NOW(),
    )
    .unwrap();

    assert_eq!(tokens.access_token, "at");
    assert_eq!(tokens.refresh_token, "rt");
    assert_eq!(tokens.expires_at, datetime!(2026-07-03 13:00:00 Z));
    assert_eq!(tokens.identity, AccountIdentity::default());
}

#[test]
fn test_account_identity_is_read_when_present() {
    let tokens = parse_tokens(
        r#"{
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 60,
            "account": { "uuid": "acct-1", "email_address": "jean@example.com" }
        }"#,
        NOW(),
    )
    .unwrap();

    assert_eq!(tokens.identity, AccountIdentity {
        account_id: Some("acct-1".to_owned()),
        email: Some("jean@example.com".to_owned()),
    });
}

/// An account block with empty strings carries no identity, and must not be
/// mistaken for a verified profile.
#[test]
fn test_empty_account_fields_are_not_an_identity() {
    let tokens = parse_tokens(
        r#"{
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 60,
            "account": { "uuid": "", "email_address": "" }
        }"#,
        NOW(),
    )
    .unwrap();

    assert_eq!(tokens.identity, AccountIdentity::default());
}

/// A refusal is terminal for the credential, while a transport failure is worth
/// another attempt; the caller marks a profile for re-login on the former only.
#[test]
fn test_only_a_refusal_is_a_rejection() {
    let rejected = OauthError::Rejected {
        status: 400,
        body: "invalid_grant".to_owned(),
    };
    assert!(rejected.is_rejection());

    let malformed = OauthError::Malformed(serde_json::from_str::<Value>("{").unwrap_err());
    assert!(!malformed.is_rejection());
}
