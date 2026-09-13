use test_log::test;

use super::*;

/// The bootstrap call talks HTTPS, which requires a TLS backend compiled into
/// `reqwest`.
/// A missing backend surfaces at client construction as an opaque `builder
/// error`, so pin it here rather than discovering it at login time.
#[test]
fn test_bootstrap_client_builds() {
    let result = bootstrap_client();
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn test_parse_bootstrap_response() {
    let identity = parse_bootstrap_response(
        r#"{"oauth_account": {"account_uuid": "uuid-1", "account_email": "a@b.c"}}"#,
    );
    assert_eq!(identity, AccountIdentity {
        account_id: Some("uuid-1".to_owned()),
        email: Some("a@b.c".to_owned()),
    });
}

#[test]
fn test_parse_bootstrap_response_tolerates_gaps() {
    // Empty strings, missing fields, and unparseable bodies all degrade to
    // a (partially) empty identity.
    let cases = [
        r#"{"oauth_account": {"account_uuid": "", "account_email": "a@b.c"}}"#,
        r#"{"oauth_account": {"account_email": "a@b.c"}}"#,
    ];
    for body in cases {
        let identity = parse_bootstrap_response(body);
        assert_eq!(identity.account_id, None, "body: {body}");
        assert_eq!(identity.email.as_deref(), Some("a@b.c"), "body: {body}");
    }

    assert_eq!(parse_bootstrap_response("{}"), AccountIdentity::default());
    assert_eq!(
        parse_bootstrap_response("not json"),
        AccountIdentity::default()
    );
}
