use chrono::TimeZone as _;

use super::*;

/// A JWT whose payload is `{"chatgpt_account_id":"acct-top",
/// "email":"jean@example.com"}`.
const TOKEN_TOP_LEVEL: &str =
    "header.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2N0LXRvcCIsImVtYWlsIjoiamVhbkBleGFtcGxlLmNvbSJ9.sig";

/// A JWT whose payload nests the account id and a residency constraint under
/// `https://api.openai.com/auth`.
const TOKEN_NESTED: &str = "header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1uZXN0ZWQiLCJjaGF0Z3B0X2NvbXB1dGVfcmVzaWRlbmN5IjoiZXUifX0.sig";

/// A JWT whose payload carries only an organization list.
const TOKEN_ORGS: &str =
    "header.eyJvcmdhbml6YXRpb25zIjpbeyJpZCI6Im9yZy1maXJzdCJ9LHsiaWQiOiJvcmctc2Vjb25kIn1dfQ.sig";

#[test]
fn test_identity_reads_top_level_account_claim() {
    let identity = identity_from_tokens(TOKEN_TOP_LEVEL, "");

    assert_eq!(identity.account_id.as_deref(), Some("acct-top"));
    assert_eq!(identity.email.as_deref(), Some("jean@example.com"));
}

#[test]
fn test_identity_reads_nested_account_claim() {
    let identity = identity_from_tokens(TOKEN_NESTED, "");

    assert_eq!(identity.account_id.as_deref(), Some("acct-nested"));
    assert_eq!(identity.email, None);
}

#[test]
fn test_identity_falls_back_to_first_organization() {
    let identity = identity_from_tokens(TOKEN_ORGS, "");

    assert_eq!(identity.account_id.as_deref(), Some("org-first"));
}

#[test]
fn test_identity_falls_back_to_access_token() {
    // A refresh response often omits the id token, leaving the access token as
    // the only source of account identity.
    let identity = identity_from_tokens("", TOKEN_TOP_LEVEL);

    assert_eq!(identity.account_id.as_deref(), Some("acct-top"));
}

#[test]
fn test_identity_is_empty_for_a_non_jwt() {
    let identity = identity_from_tokens("not-a-jwt", "also-not-a-jwt");

    assert_eq!(identity, AccountIdentity::default());
}

#[test]
fn test_residency_read_from_nested_claim() {
    assert_eq!(residency(TOKEN_NESTED).as_deref(), Some("eu"));
}

#[test]
fn test_residency_absent_without_a_constraint() {
    assert_eq!(residency(TOKEN_TOP_LEVEL), None);
}

#[test]
fn test_parse_tokens_resolves_relative_expiry() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let body = format!(
        r#"{{"access_token":"access-1","refresh_token":"refresh-1","expires_in":600,"id_token":"{TOKEN_TOP_LEVEL}"}}"#
    );

    let tokens = parse_tokens(&body, now).unwrap();

    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token, "refresh-1");
    assert_eq!(
        tokens.expires_at,
        Utc.with_ymd_and_hms(2026, 9, 9, 12, 10, 0).unwrap()
    );
    assert_eq!(tokens.identity.account_id.as_deref(), Some("acct-top"));
}

#[test]
fn test_parse_tokens_defaults_expiry_when_omitted() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let body = r#"{"access_token":"access-1","refresh_token":"refresh-1"}"#;

    let tokens = parse_tokens(body, now).unwrap();

    assert_eq!(
        tokens.expires_at,
        Utc.with_ymd_and_hms(2026, 9, 9, 13, 0, 0).unwrap()
    );
}

#[test]
fn test_authorize_url_carries_pkce_and_originator() {
    let url = authorize_url(
        "challenge-1",
        "state-1",
        "http://localhost:1455/auth/callback",
    );

    assert_eq!(
        url,
        "https://auth.openai.com/oauth/authorize?response_type=code\
         &client_id=app_EMoamEEZ73f0CkXaXp7hrann\
         &redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback\
         &scope=openid%20profile%20email%20offline_access\
         &code_challenge=challenge-1&code_challenge_method=S256\
         &id_token_add_organizations=true&codex_cli_simplified_flow=true\
         &state=state-1&originator=jp"
    );
}

#[test]
fn test_exchange_form_is_form_encoded_pairs() {
    let form = exchange_form(
        "code-1",
        "verifier-1",
        "http://localhost:1455/auth/callback",
    );

    assert_eq!(form, vec![
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("code".to_owned(), "code-1".to_owned()),
        (
            "redirect_uri".to_owned(),
            "http://localhost:1455/auth/callback".to_owned()
        ),
        (
            "client_id".to_owned(),
            "app_EMoamEEZ73f0CkXaXp7hrann".to_owned()
        ),
        ("code_verifier".to_owned(), "verifier-1".to_owned()),
    ]);
}

#[test]
fn test_refresh_form_omits_the_verifier() {
    let form = refresh_form("refresh-1");

    assert_eq!(form, vec![
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("refresh_token".to_owned(), "refresh-1".to_owned()),
        (
            "client_id".to_owned(),
            "app_EMoamEEZ73f0CkXaXp7hrann".to_owned()
        ),
    ]);
}

#[test]
fn test_pkce_challenge_is_the_sha256_of_the_verifier() {
    let pkce = Pkce::generate();

    let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()));
    assert_eq!(pkce.challenge, expected);
    assert_ne!(pkce.verifier, pkce.challenge);
}

#[test]
fn test_device_poll_interval_adds_a_safety_margin() {
    let device = DeviceAuth {
        device_auth_id: "dev-1".to_owned(),
        user_code: "ABCD-1234".to_owned(),
        interval: "5".to_owned(),
    };

    assert_eq!(device.poll_interval(), Duration::from_secs(8));
}

#[test]
fn test_device_poll_interval_survives_a_missing_interval() {
    let device = DeviceAuth {
        device_auth_id: "dev-1".to_owned(),
        user_code: "ABCD-1234".to_owned(),
        interval: String::new(),
    };

    assert_eq!(device.poll_interval(), Duration::from_secs(8));
}

#[test]
fn test_rejection_is_distinguished_from_transport_failure() {
    let rejected = OauthError::Rejected {
        status: 400,
        body: "bad grant".to_owned(),
    };
    assert!(rejected.is_rejection());

    let timeout = OauthError::DeviceTimeout;
    assert!(!timeout.is_rejection());
}
