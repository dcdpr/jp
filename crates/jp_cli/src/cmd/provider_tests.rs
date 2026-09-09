use std::{collections::BTreeMap, sync::Arc};

use camino_tempfile::Utf8TempDir;
use datetime_literal::datetime;
use jp_credentials::{FsCredentialBackend, PROVIDER_ANTHROPIC};
use jp_printer::{OutputFormat, Printer};
use jp_storage::resource_lock::FsResourceLocker;
use test_log::test;

use super::*;

fn anthropic_target() -> AuthTarget {
    AuthTarget {
        provider: ProviderId::Anthropic,
    }
}

fn token_profile(token: &str, account_id: Option<&str>) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Token {
            token: token.to_owned(),
        },
        account_id: account_id.map(str::to_owned),
        email: account_id.map(|_| "jean@example.com".to_owned()),
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    }
}

fn store_at(dir: &Utf8TempDir) -> CredentialStore {
    CredentialStore::new(
        Arc::new(FsCredentialBackend::new(
            dir.path().join("credentials.json"),
        )),
        Arc::new(FsResourceLocker::new(dir.path().to_owned())),
    )
}

#[test]
fn test_auth_target_parses_only_supported_providers() {
    assert_eq!(
        "llm.anthropic".parse::<AuthTarget>().unwrap().provider,
        ProviderId::Anthropic
    );

    // Real providers without stored-credential support, unknown providers,
    // unknown categories, and non-dotted forms are all rejected.
    for input in ["llm.openai", "llm.bogus", "tts.anthropic", "anthropic"] {
        assert!(input.parse::<AuthTarget>().is_err(), "input: {input:?}");
    }
}

#[test]
fn test_auth_target_store_key_matches_resolver_key() {
    // `resolve` looks profiles up under this constant; the auth commands
    // derive the same key from the provider id. The two must agree or
    // logins would store profiles resolution never finds.
    assert_eq!(anthropic_target().store_key(), PROVIDER_ANTHROPIC);
}

#[test]
fn test_list_empty_store_renders_empty_json_array() {
    let dir = Utf8TempDir::new().unwrap();
    let (printer, out, _err) = Printer::memory(OutputFormat::Json);

    List {}.run(&store_at(&dir), &printer).unwrap();
    printer.shutdown();

    assert_eq!(out.lock().trim(), "[]");
}

#[test]
fn test_list_renders_profiles_and_states_as_json() {
    let dir = Utf8TempDir::new().unwrap();
    let store = store_at(&dir);

    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "personal",
                token_profile("sk-a", Some("uuid-1")),
            );
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "ci",
                token_profile("sk-b", None),
            );
            Ok(())
        })
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Json);
    List {}.run(&store, &printer).unwrap();
    printer.shutdown();

    let expected = r#"[{"Provider":"llm.anthropic","Profile":"ci","Type":"token","Account":"","State":"unverified (usable)"},{"Provider":"llm.anthropic","Profile":"personal","Type":"token","Account":"jean@example.com","State":"valid"}]"#;
    assert_eq!(out.lock().trim(), expected);
}

#[test]
fn test_list_renders_markdown_table_when_piped() {
    let dir = Utf8TempDir::new().unwrap();
    let store = store_at(&dir);

    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "personal",
                token_profile("sk-a", Some("uuid-1")),
            );
            Ok(())
        })
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    List {}.run(&store, &printer).unwrap();
    printer.shutdown();

    let output = out.lock().clone();
    assert!(
        output.starts_with("| Provider "),
        "expected a markdown table, got: {output}"
    );
    assert!(output.contains("| llm.anthropic |"), "{output}");
}

#[test]
fn test_logout_removes_sole_profile_without_name() {
    let dir = Utf8TempDir::new().unwrap();
    let store = store_at(&dir);

    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "personal",
                token_profile("sk-a", Some("uuid-1")),
            );
            Ok(())
        })
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Logout {
        target: anthropic_target(),
        profile: None,
    }
    .run(&store, &printer)
    .unwrap();
    printer.shutdown();

    assert_eq!(
        out.lock().trim(),
        r#"Removed llm.anthropic profile "personal"."#
    );
    assert_eq!(store.load().unwrap().iter().count(), 0);
}

#[test]
fn test_logout_requires_profile_name_when_ambiguous() {
    let dir = Utf8TempDir::new().unwrap();
    let store = store_at(&dir);

    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "personal",
                token_profile("sk-a", Some("uuid-1")),
            );
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "work",
                token_profile("sk-b", Some("uuid-2")),
            );
            Ok(())
        })
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let error = Logout {
        target: anthropic_target(),
        profile: None,
    }
    .run(&store, &printer)
    .unwrap_err();
    printer.shutdown();

    assert!(error.to_string().contains("multiple profiles stored"));
    assert_eq!(store.load().unwrap().iter().count(), 2);
}

#[test]
fn test_logout_unknown_profile_names_stored_ones() {
    let dir = Utf8TempDir::new().unwrap();
    let store = store_at(&dir);

    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "personal",
                token_profile("sk-a", Some("uuid-1")),
            );
            Ok(())
        })
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let error = Logout {
        target: anthropic_target(),
        profile: Some("work".to_owned()),
    }
    .run(&store, &printer)
    .unwrap_err();
    printer.shutdown();

    let message = error.to_string();
    assert!(message.contains(r#"no stored profile "work""#), "{message}");
    assert!(message.contains("personal"), "{message}");
}

#[test]
fn test_sanitize_setup_token() {
    // Provider-supplied guidance, appended to every rejection so the user is
    // told how to obtain a token for the provider they named.
    const HINT: &str = "Run the token command and paste its value.";

    // Plain tokens pass through, trimmed.
    assert_eq!(
        sanitize_setup_token("sk-ant-oat01-abc123\n", HINT).unwrap(),
        "sk-ant-oat01-abc123"
    );

    // ANSI styling from a decorated `claude setup-token` run is stripped:
    // the escapes are exactly what made the HTTP client reject the header.
    assert_eq!(
        sanitize_setup_token("\u{1b}[32msk-ant-oat01-abc123\u{1b}[0m\n", HINT).unwrap(),
        "sk-ant-oat01-abc123"
    );

    // Empty input is named as such.
    let error = sanitize_setup_token("   \n", HINT).unwrap_err();
    assert_eq!(error, "no setup token provided on stdin");

    // The exact failure a raw `claude setup-token` pipe produces: its first
    // line is a colorized banner, whose ANSI escapes made the HTTP client
    // reject the header with an opaque `builder error`. Stripping leaves
    // the banner text, which the whitespace check names.
    let error = sanitize_setup_token("\u{1b}[1mWelcome to Claude Code v2.1.92\u{1b}[0m\n", HINT)
        .unwrap_err();
    assert!(error.contains("contains whitespace"), "{error}");
    // Every rejection carries the provider's own guidance.
    assert!(error.contains("first line of stdin"), "{error}");
    assert!(error.contains(HINT), "{error}");
    // The input is never echoed back: a valid token is a secret.
    assert!(
        !error.contains("Welcome"),
        "input must not be echoed: {error}"
    );

    // A spinner frame or other decoration left after ANSI stripping is
    // rejected with the offending character, not passed on to fail opaquely
    // inside the HTTP client.
    let error = sanitize_setup_token("\u{280b}token\n", HINT).unwrap_err();
    assert!(
        error.contains("cannot be sent in an HTTP header"),
        "{error}"
    );
    assert!(error.contains("'\u{280b}'"), "{error}");

    // A spinner overwriting itself with a lone carriage return: ANSI
    // stripping discards the `\r`, which would splice the segments into a
    // plausible-looking token, so this is rejected before stripping.
    let error = sanitize_setup_token("working\rsk-ant-oat01-abc\n", HINT).unwrap_err();
    assert!(error.contains("contains a carriage return"), "{error}");

    // A CRLF line ending is just a line ending, not an overwrite.
    assert_eq!(
        sanitize_setup_token("sk-ant-oat01-abc123\r\n", HINT).unwrap(),
        "sk-ant-oat01-abc123"
    );
}

#[test]
fn test_credential_state_variants() {
    let now = datetime!(2026-07-03 12:00:00 Z);

    let valid = token_profile("sk-a", Some("uuid-1"));
    assert_eq!(credential_state(&valid, now), "valid");

    let unverified = token_profile("sk-a", None);
    assert_eq!(credential_state(&unverified, now), "unverified (usable)");

    let mut relogin = token_profile("sk-a", None);
    relogin.needs_relogin = true;
    assert_eq!(
        credential_state(&relogin, now),
        "needs re-login, unverified"
    );

    let mut cooling = token_profile("sk-a", Some("uuid-1"));
    cooling
        .cooldowns
        .insert("opus".to_owned(), datetime!(2026-07-03 13:00:00 Z));
    assert_eq!(
        credential_state(&cooling, now),
        "cooling down until 2026-07-03 13:00:00 UTC (opus)"
    );

    let mut oauth = StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: "at".to_owned(),
            refresh_token: "rt".to_owned(),
            expires_at: datetime!(2026-07-03 11:00:00 Z),
        },
        account_id: Some("uuid-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    };
    assert_eq!(credential_state(&oauth, now), "expired");

    // A live OAuth credential reports its expiry; a static token has none
    // to report and stays plain "valid".
    oauth.secret = CredentialSecret::Oauth {
        access_token: "at".to_owned(),
        refresh_token: "rt".to_owned(),
        expires_at: datetime!(2026-07-03 13:00:00 Z),
    };
    assert_eq!(
        credential_state(&oauth, now),
        "valid (expires 2026-07-03 13:00:00 UTC)"
    );
}
