use camino_tempfile::Utf8TempDir;
use datetime_literal::datetime;
use jp_storage::resource_lock::InMemoryResourceLocker;
use test_log::test;

use super::*;

fn token_credential(token: &str) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Token {
            token: token.to_owned(),
        },
        account_id: Some("11111111-1111-1111-1111-111111111111".to_owned()),
        email: Some("jean@example.com".to_owned()),
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    }
}

fn file_store(dir: &Utf8TempDir) -> CredentialStore {
    CredentialStore::new(
        Arc::new(FsCredentialBackend::new(dir.path().join(STORE_FILENAME))),
        Arc::new(FsResourceLocker::new(dir.path().to_owned())),
    )
}

fn memory_store() -> CredentialStore {
    CredentialStore::new(
        Arc::new(InMemoryCredentialBackend::new()),
        Arc::new(InMemoryResourceLocker::new()),
    )
}

/// The stores that must behave identically through `CredentialStore`.
fn stores() -> Vec<(&'static str, CredentialStore, Option<Utf8TempDir>)> {
    let dir = Utf8TempDir::new().unwrap();
    let file = file_store(&dir);
    vec![("fs", file, Some(dir)), ("memory", memory_store(), None)]
}

#[test]
fn test_load_missing_store_is_empty() {
    for (name, store, _dir) in stores() {
        let document = store.load().unwrap();
        assert_eq!(document.iter().count(), 0, "store: {name}");
    }
}

#[test]
fn test_mutate_roundtrip() {
    for (name, store, _dir) in stores() {
        store
            .mutate(|document| {
                document.insert_profile("llm", "anthropic", "personal", token_credential("sk-a"));
                Ok(())
            })
            .unwrap();

        let document = store.load().unwrap();
        let profiles = document.profiles("llm", "anthropic").unwrap();
        assert_eq!(profiles.len(), 1, "store: {name}");
        assert_eq!(
            profiles["personal"].secret,
            CredentialSecret::Token {
                token: "sk-a".to_owned()
            },
            "store: {name}"
        );
    }
}

#[test]
fn test_mutate_error_leaves_store_untouched() {
    for (name, store, _dir) in stores() {
        store
            .mutate(|document| {
                document.insert_profile("llm", "anthropic", "personal", token_credential("sk-a"));
                Ok(())
            })
            .unwrap();

        let result = store.mutate(|document| {
            document.remove_profile("llm", "anthropic", "personal");
            Err::<(), _>(StoreError::Rejected("nope".to_owned()))
        });

        assert!(result.is_err(), "store: {name}");
        let document = store.load().unwrap();
        assert!(
            document.profiles("llm", "anthropic").is_some(),
            "store: {name}"
        );
    }
}

#[test]
fn test_remove_profile_prunes_empty_maps() {
    let mut document = StoreDocument::default();
    document.insert_profile("llm", "anthropic", "personal", token_credential("sk-a"));

    let removed = document.remove_profile("llm", "anthropic", "personal");
    assert!(removed.is_some());
    assert!(document.profiles("llm", "anthropic").is_none());
    assert_eq!(document.iter().count(), 0);

    assert!(
        document
            .remove_profile("llm", "anthropic", "personal")
            .is_none()
    );
}

#[test]
fn test_rejects_newer_schema_version() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(STORE_FILENAME),
        r#"{"version": 2, "credentials": {}}"#,
    )
    .unwrap();

    let error = file_store(&dir).load().unwrap_err();
    assert!(matches!(error, StoreError::NewerVersion { found: 2, .. }));
}

#[test]
fn test_rejects_malformed_store() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(dir.path().join(STORE_FILENAME), "not json").unwrap();

    let error = file_store(&dir).load().unwrap_err();
    assert!(matches!(error, StoreError::Malformed { .. }));
}

#[test]
fn test_wire_format_matches_rfd_shape() {
    // The exact document shape from RFD 090's Credential store section.
    let raw = r#"{
        "version": 1,
        "credentials": {
            "llm": {
                "anthropic": {
                    "personal": {
                        "type": "oauth",
                        "access_token": "at",
                        "refresh_token": "rt",
                        "expires_at": "2026-07-03T12:00:00Z",
                        "account_id": "11111111-1111-1111-1111-111111111111",
                        "email": "jean@example.com",
                        "cooldowns": {}
                    },
                    "ci": {
                        "type": "token",
                        "token": "sk-ant-xxx",
                        "account_id": null,
                        "email": null,
                        "cooldowns": {}
                    }
                }
            }
        }
    }"#;

    let document: StoreDocument = serde_json::from_str(raw).unwrap();
    let profiles = document.profiles("llm", "anthropic").unwrap();

    assert_eq!(profiles["personal"].secret, CredentialSecret::Oauth {
        access_token: "at".to_owned(),
        refresh_token: "rt".to_owned(),
        expires_at: datetime!(2026-07-03 12:00:00 Z),
    });
    assert_eq!(profiles["ci"].secret, CredentialSecret::Token {
        token: "sk-ant-xxx".to_owned()
    });
    assert_eq!(profiles["ci"].account_id, None);
    assert!(!profiles["ci"].needs_relogin);

    // Round-trips without dropping fields.
    let serialized = serde_json::to_value(&document).unwrap();
    let reparsed: StoreDocument = serde_json::from_value(serialized).unwrap();
    assert_eq!(reparsed, document);
}

#[test]
fn test_active_cooldown_scoping() {
    let mut credential = token_credential("sk-a");
    let now = datetime!(2026-07-03 12:00:00 Z);
    let future = datetime!(2026-07-03 13:00:00 Z);
    let past = datetime!(2026-07-03 11:00:00 Z);

    // No cooldowns: nothing matches.
    assert_eq!(credential.active_cooldown("claude-opus-4-6", now), None);

    // A model-family scope only blocks that family.
    credential.cooldowns.insert("opus".to_owned(), future);
    assert_eq!(
        credential.active_cooldown("claude-opus-4-6", now),
        Some(("opus", future))
    );
    assert_eq!(credential.active_cooldown("claude-haiku-4-5", now), None);

    // An expired cooldown no longer matches.
    credential.cooldowns.insert("opus".to_owned(), past);
    assert_eq!(credential.active_cooldown("claude-opus-4-6", now), None);

    // The account scope blocks every model.
    credential.cooldowns.insert("account".to_owned(), future);
    assert_eq!(
        credential.active_cooldown("claude-haiku-4-5", now),
        Some(("account", future))
    );
}

#[test]
fn test_cooldown_until_policy() {
    let now = datetime!(2026-07-03 12:00:00 Z);

    // No reported timing: the fixed default applies.
    assert_eq!(cooldown_until(None, now), now + DEFAULT_COOLDOWN);

    // Timing already in the past says nothing about the future window.
    let past = datetime!(2026-07-03 11:00:00 Z);
    assert_eq!(cooldown_until(Some(past), now), now + DEFAULT_COOLDOWN);

    // Reported timing wins when it is in the future.
    let reset = datetime!(2026-07-03 17:00:00 Z);
    assert_eq!(cooldown_until(Some(reset), now), reset);

    // A misparsed far-future timestamp cannot brick the profile.
    let absurd = datetime!(2030-01-01 00:00:00 Z);
    assert_eq!(cooldown_until(Some(absurd), now), now + MAX_COOLDOWN);
}

#[test]
fn test_record_cooldown_and_relogin() {
    for (name, store, _dir) in stores() {
        store
            .mutate(|document| {
                document.insert_profile("llm", "anthropic", "personal", token_credential("sk-a"));
                Ok(())
            })
            .unwrap();

        let until = datetime!(2026-07-03 13:00:00 Z);
        let found = store
            .record_cooldown("llm", "anthropic", "personal", "opus", until)
            .unwrap();
        assert!(found, "store: {name}");

        let document = store.load().unwrap();
        let credential = &document.profiles("llm", "anthropic").unwrap()["personal"];
        assert_eq!(credential.cooldowns["opus"], until, "store: {name}");
        assert!(!credential.needs_relogin, "store: {name}");

        // A shorter window never shortens a recorded cooldown: a process
        // that saw the longer one must not be undercut.
        let shorter = datetime!(2026-07-03 12:30:00 Z);
        store
            .record_cooldown("llm", "anthropic", "personal", "opus", shorter)
            .unwrap();
        let document = store.load().unwrap();
        assert_eq!(
            document.profiles("llm", "anthropic").unwrap()["personal"].cooldowns["opus"],
            until,
            "store: {name}"
        );

        // Re-login state is independent of cooldowns.
        store
            .mark_needs_relogin("llm", "anthropic", "personal")
            .unwrap();
        let document = store.load().unwrap();
        let credential = &document.profiles("llm", "anthropic").unwrap()["personal"];
        assert!(credential.needs_relogin, "store: {name}");
        assert_eq!(credential.cooldowns["opus"], until, "store: {name}");

        // A chain entry with no stored profile has nothing to record
        // against, and that is not an error.
        let found = store
            .record_cooldown("llm", "anthropic", "absent", "account", until)
            .unwrap();
        assert!(!found, "store: {name}");
    }
}

#[cfg(unix)]
#[test]
fn test_store_file_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = Utf8TempDir::new().unwrap();

    file_store(&dir)
        .mutate(|document| {
            document.insert_profile("llm", "anthropic", "personal", token_credential("sk-a"));
            Ok(())
        })
        .unwrap();

    let mode = std::fs::metadata(dir.path().join(STORE_FILENAME))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
