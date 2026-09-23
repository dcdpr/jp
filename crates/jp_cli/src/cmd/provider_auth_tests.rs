use std::{
    error::Error as StdError,
    io,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use camino_tempfile::Utf8TempDir;
use clap::{Parser as _, error::ErrorKind};
use jp_credentials::{FsCredentialBackend, PROVIDER_ANTHROPIC};
use jp_llm::credential::ExternalAuth;
use jp_printer::OutputFormat;
use jp_storage::resource_lock::FsResourceLocker;

use super::*;
use crate::{Cli, cmd::Commands};

#[derive(Clone, Default)]
struct FakeAuth {
    calls: Arc<Mutex<Vec<(String, Utf8PathBuf)>>>,
    login_fails: bool,
    logout_fails: bool,
    signed_out: bool,
    status_fails: bool,
}

#[async_trait]
impl ProviderAuth for FakeAuth {
    fn external_auth(&self) -> Option<&dyn ExternalAuth> {
        Some(self)
    }
    fn setup_token_hint(&self) -> &'static str {
        panic!("external login must not request a token")
    }
    async fn recover_identity(
        &self,
        _: &str,
    ) -> Result<AccountIdentity, Box<dyn StdError + Send + Sync>> {
        panic!("external login must not read tokens")
    }
}

#[async_trait]
impl ExternalAuth for FakeAuth {
    fn directory_name(&self) -> &'static str {
        "claude"
    }
    async fn login(
        &self,
        directory: &Utf8Path,
    ) -> Result<AccountIdentity, Box<dyn StdError + Send + Sync>> {
        self.calls
            .lock()
            .unwrap()
            .push(("login".into(), directory.to_owned()));
        if self.login_fails {
            return Err(io::Error::other("login refused").into());
        }
        Ok(AccountIdentity {
            account_id: None,
            email: Some("first@example.com".into()),
        })
    }
    async fn status(
        &self,
        directory: &Utf8Path,
    ) -> Result<Option<AccountIdentity>, Box<dyn StdError + Send + Sync>> {
        self.calls
            .lock()
            .unwrap()
            .push(("status".into(), directory.to_owned()));
        if self.status_fails {
            return Err(io::Error::other("runtime unavailable").into());
        }
        Ok((!self.signed_out).then(|| AccountIdentity {
            account_id: None,
            email: Some("first@example.com".into()),
        }))
    }
    async fn logout(&self, directory: &Utf8Path) -> Result<(), Box<dyn StdError + Send + Sync>> {
        self.calls
            .lock()
            .unwrap()
            .push(("logout".into(), directory.to_owned()));
        if self.logout_fails {
            return Err(io::Error::other("logout refused").into());
        }
        Ok(())
    }
}

fn store_at(root: &Utf8Path) -> CredentialStore {
    CredentialStore::new(
        Arc::new(FsCredentialBackend::new(root.join("credentials.json"))),
        Arc::new(FsResourceLocker::new(root)),
    )
}

fn login_args(name: &str) -> Login {
    let cli = Cli::try_parse_from([
        "jp",
        "provider",
        "llm",
        "auth",
        "login",
        "anthropic",
        "--name",
        name,
    ])
    .unwrap();
    let Commands::Provider(provider) = cli.command else {
        panic!("expected provider command")
    };
    let ProviderCmd::Llm(llm) = provider.command;
    let LlmCmd::Auth(auth) = llm.command;
    let AuthCmd::Login(login) = auth.command else {
        panic!("expected login command")
    };
    login
}

fn registration(directory: &Utf8Path) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::External {
            directory: directory.to_owned(),
        },
        account_id: None,
        email: Some("first@example.com".into()),
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
        generation: 0,
    }
}

#[tokio::test]
async fn default_login_registers_runtime_directory_without_tokens() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let auth = FakeAuth::default();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    login_args("sub")
        .run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap();
    printer.shutdown();
    let directory = root.path().join("data/claude/sub");
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "login".into(),
        directory.clone()
    )]);
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub"],
        registration(&directory)
    );
    assert_eq!(
        out.lock().as_str(),
        "Logged in to anthropic as \"sub\" (first@example.com).\n"
    );
}

#[tokio::test]
async fn relogin_preserves_the_registered_directory_when_data_root_changes() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("original/sub");
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registration(&directory),
            );
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth::default();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    login_args("sub")
        .run_with_auth(
            &store,
            &printer,
            &auth,
            Some(&root.path().join("different")),
        )
        .await
        .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "login".into(),
        directory
    )]);
}

#[tokio::test]
async fn failed_login_keeps_existing_registration() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    let existing = registration(&directory);
    store
        .mutate(|document| {
            document.insert_profile(CATEGORY_LLM, PROVIDER_ANTHROPIC, "sub", existing.clone());
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth {
        login_fails: true,
        ..Default::default()
    };
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let error = login_args("sub")
        .run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap_err();
    printer.shutdown();
    assert_eq!(error.message.as_deref(), Some("login refused"));
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "login".into(),
        directory
    )]);
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub"],
        existing
    );
    assert_eq!(out.lock().as_str(), "");
}

#[tokio::test]
async fn logout_clears_only_the_selected_runtime_login() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let first = root.path().join("data/claude/sub");
    let second = root.path().join("data/claude/sub2");
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registration(&first),
            );
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub2",
                registration(&second),
            );
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth::default();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    Logout {
        target: AuthTarget {
            provider: ProviderId::Anthropic,
        },
        name: Some("sub2".into()),
    }
    .run_with_auth(&store, &printer, |_| Some(Box::new(auth.clone())))
    .await
    .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![("logout".into(), second)]);
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap(),
        &BTreeMap::from([("sub".into(), registration(&first))])
    );
    assert_eq!(out.lock().as_str(), "Removed anthropic profile \"sub2\".\n");
}

#[tokio::test]
async fn failed_logout_retains_registration_for_retry() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    let existing = registration(&directory);
    store
        .mutate(|document| {
            document.insert_profile(CATEGORY_LLM, PROVIDER_ANTHROPIC, "sub", existing.clone());
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth {
        logout_fails: true,
        ..Default::default()
    };
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let error = Logout {
        target: AuthTarget {
            provider: ProviderId::Anthropic,
        },
        name: None,
    }
    .run_with_auth(&store, &printer, |_| Some(Box::new(auth.clone())))
    .await
    .unwrap_err();
    printer.shutdown();
    assert_eq!(error.message.as_deref(), Some("logout refused"));
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "logout".into(),
        directory
    )]);
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub"],
        existing
    );
    assert_eq!(out.lock().as_str(), "");
}

#[tokio::test]
async fn list_checks_runtime_login_without_exposing_its_mechanism() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registration(&directory),
            );
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth::default();
    let (printer, out, err) = Printer::memory(OutputFormat::Json);
    List {}
        .run_with_auth(&store, &[], &printer, |_| Some(Box::new(auth.clone())))
        .await
        .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "status".into(),
        directory
    )]);
    assert_eq!(
        out.lock().as_str(),
        "[{\"provider\":\"anthropic\",\"name\":\"sub\",\"kind\":\"subscription\",\"state\":\"\
         valid\",\"verified\":true,\"expires_in_secs\":null,\"cooldowns\":[]}]\n"
    );
    assert_eq!(err.lock().as_str(), "");
}

#[tokio::test]
async fn unsafe_names_are_rejected_before_runtime_login() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let auth = FakeAuth::default();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let error = login_args("../other")
        .run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap_err();
    printer.shutdown();
    assert_eq!(
        error.message.as_deref(),
        Some("subscription names must contain only ASCII letters, digits, '-' or '_'")
    );
    assert!(auth.calls.lock().unwrap().is_empty());
    assert_eq!(store.load().unwrap().iter().count(), 0);
}

#[tokio::test]
async fn signed_out_subscription_is_not_reported_as_usable() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registration(&directory),
            );
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth {
        signed_out: true,
        ..Default::default()
    };
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    List {}
        .run_with_auth(&store, &[], &printer, |_| Some(Box::new(auth.clone())))
        .await
        .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "status".into(),
        directory
    )]);
    assert_eq!(
        out.lock().as_str(),
        "[{\"provider\":\"anthropic\",\"name\":\"sub\",\"kind\":\"subscription\",\"state\":\"\
         needs_relogin\",\"verified\":true,\"expires_in_secs\":null,\"cooldowns\":[]}]\n"
    );
}

#[tokio::test]
async fn unavailable_runtime_reports_unknown_state_without_removing_login() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    let existing = registration(&directory);
    store
        .mutate(|document| {
            document.insert_profile(CATEGORY_LLM, PROVIDER_ANTHROPIC, "sub", existing.clone());
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth {
        status_fails: true,
        ..Default::default()
    };
    let (printer, out, err) = Printer::memory(OutputFormat::Json);
    List {}
        .run_with_auth(&store, &[], &printer, |_| Some(Box::new(auth.clone())))
        .await
        .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "status".into(),
        directory
    )]);
    assert_eq!(
        out.lock().as_str(),
        "[{\"provider\":\"anthropic\",\"name\":\"sub\",\"kind\":\"subscription\",\"state\":\"\
         unavailable\",\"verified\":true,\"expires_in_secs\":null,\"cooldowns\":[]}]\n"
    );
    assert_eq!(
        err.lock().as_str(),
        "{\"message\":\"could not check anthropic subscription \\\"sub\\\": runtime \
         unavailable\"}\n"
    );
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub"],
        existing
    );
}

#[tokio::test]
async fn explicit_existing_directory_is_registered_without_relocation() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("existing/sub2");
    let mut args = login_args("sub2");
    args.config_dir = Some(directory.clone());
    let auth = FakeAuth::default();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    args.run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap();
    printer.shutdown();
    assert_eq!(*auth.calls.lock().unwrap(), vec![(
        "login".into(),
        directory.clone()
    )]);
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub2"],
        registration(&directory)
    );
}

#[tokio::test]
async fn two_names_cannot_share_a_registered_login_directory() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let directory = root.path().join("data/claude/sub");
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registration(&directory),
            );
            Ok(())
        })
        .unwrap();
    let auth = FakeAuth::default();
    let mut args = login_args("sub2");
    args.config_dir = Some(directory);
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let error = args
        .run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap_err();
    printer.shutdown();
    assert_eq!(
        error.message.as_deref(),
        Some("login directory is already registered as \"sub\"")
    );
    assert!(auth.calls.lock().unwrap().is_empty());
}

#[test]
fn non_interactive_login_fails_before_opening_the_store_or_runtime() {
    let cli = Cli::try_parse_from([
        "jp",
        "provider",
        "llm",
        "auth",
        "login",
        "anthropic",
        "--name",
        "sub",
    ])
    .unwrap();
    let Commands::Provider(provider) = cli.command else {
        panic!("expected provider command")
    };
    let (printer, out, err) = Printer::memory(OutputFormat::Text);
    let error = provider.run(&printer, true).unwrap_err();
    printer.shutdown();
    assert_eq!(
        error.message.as_deref(),
        Some("login requires user interaction; remove --no-interactive to sign in")
    );
    assert_eq!(out.lock().as_str(), "");
    assert_eq!(err.lock().as_str(), "");
}

#[test]
fn direct_login_requires_a_setup_token() {
    let error = Cli::try_parse_from([
        "jp",
        "provider",
        "llm",
        "auth",
        "login",
        "anthropic",
        "--direct",
    ])
    .err()
    .unwrap();
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    assert!(
        Cli::try_parse_from([
            "jp",
            "provider",
            "llm",
            "auth",
            "login",
            "anthropic",
            "--direct",
            "--setup-token"
        ])
        .is_ok()
    );
}

#[tokio::test]
async fn token_login_requires_explicit_direct_opt_in() {
    let root = Utf8TempDir::new().unwrap();
    let store = store_at(root.path());
    let auth = FakeAuth::default();
    let mut args = login_args("sub");
    args.setup_token = true;
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let error = args
        .run_with_auth(&store, &printer, &auth, Some(root.path()))
        .await
        .unwrap_err();
    printer.shutdown();
    assert_eq!(
        error.message.as_deref(),
        Some(
            "Anthropic login uses Claude Code; direct token login requires --direct --setup-token"
        )
    );
    assert!(auth.calls.lock().unwrap().is_empty());
    assert_eq!(store.load().unwrap().iter().count(), 0);
}
