use chrono::TimeZone as _;

use super::*;

/// A JWT whose payload is `{"chatgpt_account_id":"acct-top",
/// "email":"jean@example.com"}`.
const TOKEN_TOP_LEVEL: &str =
    "header.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2N0LXRvcCIsImVtYWlsIjoiamVhbkBleGFtcGxlLmNvbSJ9.sig";

fn path() -> Utf8PathBuf {
    Utf8PathBuf::from("/home/test/.codex/auth.json")
}

#[test]
fn test_parse_reads_the_token_pair_and_identity() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let raw = format!(
        r#"{{"OPENAI_API_KEY":null,"auth_mode":"chatgpt","last_refresh":"2026-09-09T11:00:00Z",
            "tokens":{{"access_token":"access-1","refresh_token":"refresh-1",
                       "id_token":"{TOKEN_TOP_LEVEL}","account_id":"acct-file"}}}}"#
    );

    let imported = parse_codex_auth(&raw, &path(), now).unwrap();

    assert_eq!(imported.access_token, "access-1");
    assert_eq!(imported.refresh_token, "refresh-1");
    // The id token's claim wins over the file's own `account_id` field.
    assert_eq!(imported.identity.account_id.as_deref(), Some("acct-top"));
    assert_eq!(imported.identity.email.as_deref(), Some("jean@example.com"));
}

#[test]
fn test_parse_falls_back_to_the_files_account_id() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let raw = r#"{"tokens":{"access_token":"access-1","refresh_token":"refresh-1",
                            "id_token":"","account_id":"acct-file"}}"#;

    let imported = parse_codex_auth(raw, &path(), now).unwrap();

    assert_eq!(imported.identity.account_id.as_deref(), Some("acct-file"));
}

#[test]
fn test_parse_marks_the_credential_due_for_refresh() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let raw = r#"{"last_refresh":"2026-09-09T11:00:00Z",
                  "tokens":{"access_token":"a","refresh_token":"r","id_token":""}}"#;

    let imported = parse_codex_auth(raw, &path(), now).unwrap();

    // Strictly before `last_refresh`, so resolution refreshes on first use
    // rather than trusting a token whose real expiry the file never recorded.
    assert_eq!(
        imported.expires_at,
        Utc.with_ymd_and_hms(2026, 9, 9, 10, 59, 59).unwrap()
    );
    assert!(imported.expires_at < now);
}

#[test]
fn test_parse_rejects_an_api_key_only_file() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let raw = r#"{"OPENAI_API_KEY":"sk-test","auth_mode":"apikey","tokens":null}"#;

    let error = parse_codex_auth(raw, &path(), now).unwrap_err();

    assert!(
        matches!(error, ImportError::NoTokens { .. }),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("run `codex login`"));
}

#[test]
fn test_parse_rejects_a_file_without_a_refresh_token() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
    let raw = r#"{"tokens":{"access_token":"access-1","refresh_token":"","id_token":""}}"#;

    let error = parse_codex_auth(raw, &path(), now).unwrap_err();

    assert!(matches!(error, ImportError::NoTokens { .. }));
}

#[test]
fn test_parse_reports_the_path_on_malformed_json() {
    let now = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();

    let error = parse_codex_auth("not json", &path(), now).unwrap_err();

    assert!(matches!(error, ImportError::Parse { .. }));
    assert!(error.to_string().contains("/home/test/.codex/auth.json"));
}

#[tokio::test]
async fn test_recover_identity_decodes_the_access_token() {
    let identity = OpenaiAuth.recover_identity(TOKEN_TOP_LEVEL).await.unwrap();

    assert_eq!(identity.account_id.as_deref(), Some("acct-top"));
}

#[tokio::test]
async fn test_recover_identity_errors_without_an_account_claim() {
    let error = OpenaiAuth.recover_identity("not-a-jwt").await.unwrap_err();

    assert!(error.to_string().contains("no ChatGPT account claim"));
}
