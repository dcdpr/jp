use std::{collections::BTreeMap, sync::Arc};

use camino_tempfile::Utf8TempDir;
use datetime_literal::datetime;
use jp_credentials::{FsCredentialBackend, PROVIDER_ANTHROPIC, PROVIDER_OPENAI};
use jp_printer::{OutputFormat, Printer};
use jp_storage::resource_lock::FsResourceLocker;
use test_log::test;

use super::*;

fn anthropic_target() -> AuthTarget {
    AuthTarget {
        provider: ProviderId::Anthropic,
    }
}

#[test]
fn test_request_query_reads_the_callback_path() {
    let request = "GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";

    assert_eq!(
        request_query(request).as_deref(),
        Some("code=abc&state=xyz")
    );
}

#[test]
fn test_request_query_ignores_other_paths() {
    // A browser preflight or a favicon probe must not be mistaken for the
    // redirect, or the login would fail on an unrelated request.
    let request = "GET /favicon.ico?x=1 HTTP/1.1\r\n\r\n";

    assert_eq!(request_query(request), None);
}

#[test]
fn test_request_query_ignores_the_callback_without_a_query() {
    let request = "GET /auth/callback HTTP/1.1\r\n\r\n";

    assert_eq!(request_query(request), None);
}

#[test]
fn test_parse_query_decodes_escaped_values() {
    let params = parse_query("code=a%2Fb&error_description=Access+denied%21&state=s");

    assert_eq!(params.get("code").unwrap(), "a/b");
    assert_eq!(params.get("error_description").unwrap(), "Access denied!");
    assert_eq!(params.get("state").unwrap(), "s");
}

#[test]
fn test_percent_decode_leaves_a_truncated_escape_alone() {
    // A malformed redirect must not panic or silently swallow bytes.
    assert_eq!(percent_decode("abc%"), "abc%");
    assert_eq!(percent_decode("%zz"), "%zz");
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
        "anthropic".parse::<AuthTarget>().unwrap().provider,
        ProviderId::Anthropic
    );

    assert_eq!(
        "openai".parse::<AuthTarget>().unwrap().provider,
        ProviderId::Openai
    );

    // A real provider with no stored-credential support, an unknown provider,
    // and the old category-qualified form are all rejected. The category lives
    // in the command path now, so a dotted target is a stale invocation rather
    // than a provider.
    for input in ["ollama", "bogus", "llm.anthropic"] {
        assert!(input.parse::<AuthTarget>().is_err(), "input: {input:?}");
    }

    // A provider that only reads an API key says where to set it, rather than
    // leaving the user to guess that logging in was the wrong idea.
    let error = "ollama".parse::<AuthTarget>().unwrap_err();
    assert!(error.contains("api_key_env"), "{error}");
}

#[test]
fn test_auth_target_store_key_matches_resolver_key() {
    // `resolve` looks profiles up under this constant; the auth commands
    // derive the same key from the provider id. The two must agree or
    // logins would store profiles resolution never finds.
    assert_eq!(anthropic_target().store_key(), PROVIDER_ANTHROPIC);
    assert_eq!(
        "openai".parse::<AuthTarget>().unwrap().store_key(),
        PROVIDER_OPENAI
    );
}

#[test]
fn test_list_empty_store_renders_empty_json_array() {
    let dir = Utf8TempDir::new().unwrap();
    let (printer, out, _err) = Printer::memory(OutputFormat::Json);

    List {}.run(&store_at(&dir), &[], &printer).unwrap();
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
    List {}.run(&store, &api_keys(), &printer).unwrap();
    printer.shutdown();

    // The payload is built for machine consumption rather than derived from
    // the table's display strings: keys are snake_case like every other JSON
    // surface, and every value is something a caller can branch on — a variant
    // name, a boolean, a number of seconds — rather than a sentence it would
    // have to parse. `select(.state == "needs_relogin")` is the point.
    let expected = serde_json::json!([
        {
            "provider": "anthropic",
            "name": "api_key",
            "kind": "api_key",
            "env": "JP_TEST_KEY_UNSET",
            "state": "unset",
        },
        {
            "provider": "anthropic",
            "name": "ci",
            "kind": "subscription",
            "mechanism": "token",
            "state": "valid",
            "verified": false,
            "expires_in_secs": Value::Null,
            "cooldowns": [],
        },
        {
            "provider": "anthropic",
            "name": "personal",
            "kind": "subscription",
            "mechanism": "token",
            "state": "valid",
            "verified": true,
            "expires_in_secs": Value::Null,
            "cooldowns": [],
        },
    ]);

    assert_eq!(
        serde_json::from_str::<Value>(out.lock().trim()).unwrap(),
        expected
    );
}

/// The API keys a list test renders, so its output never depends on the
/// machine's config.
fn api_keys() -> Vec<(String, String, String)> {
    vec![(
        "anthropic".to_owned(),
        "api_key".to_owned(),
        "JP_TEST_KEY_UNSET".to_owned(),
    )]
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
    List {}.run(&store, &api_keys(), &printer).unwrap();
    printer.shutdown();

    let output = out.lock().clone();
    assert!(
        output.starts_with("| Provider "),
        "expected a markdown table, got: {output}"
    );
    assert!(output.contains("| anthropic |"), "{output}");
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
        name: None,
    }
    .run(&store, &printer)
    .unwrap();
    printer.shutdown();

    assert_eq!(
        out.lock().trim(),
        r#"Removed anthropic profile "personal"."#
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
        name: None,
    }
    .run(&store, &printer)
    .unwrap_err();
    printer.shutdown();

    assert!(error.to_string().contains("multiple credentials stored"));
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
        name: Some("work".to_owned()),
    }
    .run(&store, &printer)
    .unwrap_err();
    printer.shutdown();

    let message = error.to_string();
    assert!(
        message.contains(r#"no stored credential "work""#),
        "{message}"
    );
    assert!(message.contains("personal"), "{message}");
}

/// The fault is marked; the rest of the state is not.
#[test]
fn test_only_the_fault_is_emphasized() {
    let styled = emphasize("ANTHROPIC_API_KEY is not set", "not set");

    assert!(styled.starts_with("ANTHROPIC_API_KEY is "), "{styled}");
    assert!(styled.contains("not set"), "{styled}");
    assert_ne!(styled, "ANTHROPIC_API_KEY is not set", "nothing was styled");
}

/// Every payload field is a value a caller can branch on, not a sentence.
#[test]
fn test_credential_state_json_carries_values_not_prose() {
    let now = datetime!(2026-07-03 12:00:00 Z);

    let mut credential = StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: "at".to_owned(),
            refresh_token: "rt".to_owned(),
            expires_at: datetime!(2026-07-03 13:00:00 Z),
        },
        account_id: Some("uuid-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    };
    credential
        .cooldowns
        .insert("account".to_owned(), datetime!(2026-07-03 12:30:00 Z));

    let json = CredentialState::read(&credential, now).to_json();

    assert_eq!(
        json,
        serde_json::json!({
            "state": "valid",
            "verified": true,
            "expires_in_secs": 3600,
            "cooldowns": [{ "scope": "account", "expires_in_secs": 1800 }],
        })
    );
}

/// A static token reports `null` rather than omitting the field.
#[test]
fn test_a_token_credential_reports_no_expiry() {
    let now = datetime!(2026-07-03 12:00:00 Z);
    let json = CredentialState::read(&token_profile("sk-a", None), now).to_json();

    assert_eq!(
        json,
        serde_json::json!({
            "state": "valid",
            "verified": false,
            "expires_in_secs": Value::Null,
            "cooldowns": [],
        })
    );
}

/// Each lifecycle is a single lowercase word.
#[test]
fn test_lifecycle_variants_are_single_words() {
    let now = datetime!(2026-07-03 12:00:00 Z);

    let mut relogin = token_profile("sk-a", Some("uuid-1"));
    relogin.needs_relogin = true;
    assert_eq!(
        CredentialState::read(&relogin, now).to_json()["state"],
        "needs_relogin"
    );

    let expired = StoredCredential {
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
    assert_eq!(
        CredentialState::read(&expired, now).to_json()["state"],
        "expired"
    );
}

/// An API key's state is a variant, distinct from its prose form.
#[test]
fn test_key_state_variants_are_single_words() {
    assert_eq!(KeyState::Ok.as_str(), "ok");
    assert_eq!(KeyState::Unset.as_str(), "unset");
    assert_eq!(KeyState::Empty.as_str(), "empty");

    // The prose form stays the table's business.
    assert_eq!(KeyState::Unset.to_prose("FOO"), "FOO is not set");
    assert_eq!(KeyState::Empty.to_prose("FOO"), "FOO is empty");
    assert_eq!(KeyState::Ok.to_prose("FOO"), "FOO is set");
}

/// An empty variable reads as `empty`, not as `not set`.
#[test]
fn test_an_empty_variable_reads_as_empty_rather_than_unset() {
    let styled = emphasize("DEEPSEEK_API_KEY is empty", "empty");

    assert!(styled.starts_with("DEEPSEEK_API_KEY is "), "{styled}");
    assert_ne!(styled, "DEEPSEEK_API_KEY is empty", "nothing was styled");
}

/// A working credential is not marked, however much its state says.
#[test]
fn test_a_working_credential_is_not_marked() {
    for state in [
        "valid",
        "valid (expires in 9days)",
        "unverified (usable)",
        "cooling down for 1h (account)",
    ] {
        assert_eq!(
            highlight_faults(state),
            state,
            "unexpectedly marked: {state}"
        );
    }
}

/// Both faults are marked, including when one state carries two of them.
#[test]
fn test_every_fault_is_marked() {
    for state in ["expired", "needs re-login", "needs re-login, unverified"] {
        assert_ne!(
            highlight_faults(state),
            state,
            "fault went unmarked: {state}"
        );
    }
}

/// Whole units only, and the sign dropped.
#[test]
fn test_relative_renders_whole_units() {
    for (offset, expected) in [
        (chrono::Duration::seconds(45), "45s"),
        (chrono::Duration::minutes(9), "9m"),
        (chrono::Duration::hours(10), "10h"),
        (chrono::Duration::days(10), "10days"),
    ] {
        assert_eq!(humanize(offset.num_seconds()), expected, "offset: {offset}");
    }

    // A magnitude, not a direction: the caller supplies the direction.
    assert_eq!(humanize(-chrono::Duration::hours(3).num_seconds()), "3h");
}

/// The table reports an expiry as a duration, not a timestamp.
#[test]
fn test_credential_state_reports_expiry_as_a_duration() {
    let now = "2026-09-09T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

    let credential = StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: "at".to_owned(),
            refresh_token: "rt".to_owned(),
            expires_at: now + chrono::Duration::days(10),
        },
        account_id: Some("uuid-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    };

    assert_eq!(
        CredentialState::read(&credential, now).to_prose(),
        "valid (expires in 10days)"
    );
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
    assert_eq!(CredentialState::read(&valid, now).to_prose(), "valid");

    let unverified = token_profile("sk-a", None);
    assert_eq!(
        CredentialState::read(&unverified, now).to_prose(),
        "unverified (usable)"
    );

    let mut relogin = token_profile("sk-a", None);
    relogin.needs_relogin = true;
    assert_eq!(
        CredentialState::read(&relogin, now).to_prose(),
        "needs re-login, unverified"
    );

    let mut cooling = token_profile("sk-a", Some("uuid-1"));
    cooling
        .cooldowns
        .insert("opus".to_owned(), datetime!(2026-07-03 13:00:00 Z));
    assert_eq!(
        CredentialState::read(&cooling, now).to_prose(),
        "cooling down for 1h (opus)"
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
    assert_eq!(CredentialState::read(&oauth, now).to_prose(), "expired");

    // A live OAuth credential reports its expiry; a static token has none
    // to report and stays plain "valid".
    oauth.secret = CredentialSecret::Oauth {
        access_token: "at".to_owned(),
        refresh_token: "rt".to_owned(),
        expires_at: datetime!(2026-07-03 13:00:00 Z),
    };
    assert_eq!(
        CredentialState::read(&oauth, now).to_prose(),
        "valid (expires in 1h)"
    );
}
