//! Provider credential management: `jp provider auth login|list|logout`.
//!
//! Credentials are user-global, so these commands bypass workspace discovery
//! entirely (the same startup exception `jp init` uses) and work from any
//! directory.
//! `list` and `logout` require no TTY and are safe to script; `login` reads its
//! setup token from stdin, never from process arguments.

use std::{
    fmt,
    io::{self, BufRead as _, IsTerminal as _},
    str::FromStr,
};

use chrono::{DateTime, Utc};
use comfy_table::{Cell, Row};
use jp_config::model::id::ProviderId;
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, StoreError, StoredCredential,
};
use jp_printer::Printer;

use crate::{
    cmd::{Error, Output},
    error::error_chain,
    output::print_table,
};

/// `jp provider` subcommand group.
#[derive(Debug, clap::Args)]
pub(crate) struct Provider {
    #[command(subcommand)]
    command: ProviderCmd,
}

#[derive(Debug, clap::Subcommand)]
enum ProviderCmd {
    /// Manage stored provider credentials.
    Auth(Auth),
}

#[derive(Debug, clap::Args)]
struct Auth {
    #[command(subcommand)]
    command: AuthCmd,
}

#[derive(Debug, clap::Subcommand)]
enum AuthCmd {
    /// Log in to a provider and store the credential as a profile.
    Login(Login),

    /// List stored credential profiles and their state.
    #[command(visible_alias = "ls")]
    List(List),

    /// Remove a stored credential profile.
    Logout(Logout),
}

#[derive(Debug, clap::Args)]
struct Login {
    /// The provider to log in to.
    ///
    /// Only `llm.anthropic` is supported.
    target: AuthTarget,

    /// The profile name to store the credential under.
    #[arg(long, default_value = "default")]
    profile: String,

    /// Store a long-lived setup token read from the first line of stdin,
    /// instead of running the browser login.
    ///
    /// Run the provider's token command, then paste the value it prints at JP's
    /// prompt; the prompt names the command and the token's shape.
    /// A token command that runs interactively cannot be the source of a pipe,
    /// since a pipeline's stages all start at once; piping from a
    /// non-interactive source (a clipboard tool, a file) works.
    #[arg(long)]
    setup_token: bool,
}

#[derive(Debug, clap::Args)]
struct List {}

#[derive(Debug, clap::Args)]
struct Logout {
    /// The provider to log out of.
    ///
    /// Only `llm.anthropic` is supported.
    target: AuthTarget,

    /// The profile to remove.
    ///
    /// When omitted, removes the sole stored profile.
    #[arg(long)]
    profile: Option<String>,
}

/// A provider that supports stored credentials, as `<category>.<provider>`.
///
/// Parsing accepts exactly the providers for which [`jp_llm::provider_auth`]
/// reports credential mechanics; the store key is derived from the provider id.
#[derive(Debug, Clone, Copy)]
struct AuthTarget {
    provider: ProviderId,
}

impl AuthTarget {
    /// The provider's key in the credential store.
    fn store_key(self) -> String {
        self.provider.to_string()
    }
}

impl fmt::Display for AuthTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{CATEGORY_LLM}.{}", self.provider)
    }
}

impl FromStr for AuthTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((category, provider)) = s.split_once('.') else {
            return Err(format!(
                "invalid provider {s:?}: expected `<category>.<provider>`, e.g. `llm.anthropic`"
            ));
        };

        if category != CATEGORY_LLM {
            return Err(format!(
                "unsupported category {category:?}: only `llm` providers store credentials"
            ));
        }

        let provider: ProviderId = provider
            .parse()
            .map_err(|_| format!("unknown provider {provider:?}"))?;

        if jp_llm::provider_auth(provider).is_none() {
            return Err(format!(
                "provider `{s}` does not support stored credentials"
            ));
        }

        Ok(Self { provider })
    }
}

impl Provider {
    /// Run the command against the user-global credential store.
    ///
    /// Runs before workspace discovery; only `login` needs an async runtime
    /// (for identity recovery), built here on demand.
    pub(crate) fn run(&self, printer: &Printer) -> Output {
        let ProviderCmd::Auth(auth) = &self.command;
        let store = CredentialStore::file_default().map_err(|e| store_error(&e))?;

        match &auth.command {
            AuthCmd::Login(args) => {
                let runtime = crate::build_runtime(None, "jp-provider-auth")
                    .map_err(crate::cmd::Error::from)?;
                runtime.block_on(args.run(&store, printer))
            }
            AuthCmd::List(args) => args.run(&store, printer),
            AuthCmd::Logout(args) => args.run(&store, printer),
        }
    }
}

impl Login {
    async fn run(&self, store: &CredentialStore, printer: &Printer) -> Output {
        let auth = jp_llm::provider_auth(self.target.provider)
            .expect("AuthTarget parsing guarantees stored-credential support");

        if !self.setup_token {
            return Err(Error::from(format!(
                "browser login is not implemented yet; pass a setup token on stdin with \
                 `--setup-token`. {}",
                auth.setup_token_hint()
            )));
        }

        if self.profile.is_empty() || self.profile.chars().any(char::is_whitespace) {
            return Err(Error::from(format!(
                "invalid profile name {:?}: must be non-empty and contain no whitespace",
                self.profile
            )));
        }

        let token = read_setup_token(printer, auth.setup_token_hint())?;

        // Best-effort identity recovery: a failure stores the profile
        // unverified instead of blocking the login.
        let identity = auth.recover_identity(&token).await;

        let profile = self.profile.clone();
        let target = self.target;
        let (account_id, email, recovery_error) = match identity {
            Ok(identity) => (identity.account_id, identity.email, None),
            Err(error) => (None, None, Some(error_chain(error.as_ref()))),
        };

        store
            .mutate(|document| {
                // Refuse to store the same account under two profiles,
                // keyed on the account UUID; without a UUID the check is
                // skipped and the duplicate check runs if the identity is
                // recovered later.
                if let Some(account_id) = &account_id
                    && let Some(profiles) = document.profiles(CATEGORY_LLM, &target.store_key())
                    && let Some((existing, _)) = profiles.iter().find(|(name, credential)| {
                        **name != profile && credential.account_id.as_deref() == Some(account_id)
                    })
                {
                    return Err(StoreError::Rejected(format!(
                        "account {account_id} is already stored as profile {existing:?}; run `jp \
                         provider auth logout {target} --profile {existing}` first, or use that \
                         profile"
                    )));
                }

                document.insert_profile(
                    CATEGORY_LLM,
                    &target.store_key(),
                    &profile,
                    StoredCredential {
                        secret: CredentialSecret::Token {
                            token: token.clone(),
                        },
                        account_id: account_id.clone(),
                        email: email.clone(),
                        cooldowns: std::collections::BTreeMap::new(),
                        needs_relogin: false,
                    },
                );

                Ok(())
            })
            .map_err(|e| store_error(&e))?;

        match (&account_id, recovery_error) {
            (Some(account_id), _) => printer.println(format!(
                "Linked {target} profile {:?} to {} (account {account_id}).",
                self.profile,
                email.as_deref().unwrap_or("<no email>"),
            )),
            // An unattributable credential is a normal outcome, not a
            // misconfiguration: a credential whose scopes don't permit an
            // identity lookup can still authenticate requests.
            (None, error) => printer.println(format!(
                "Stored {target} profile {:?}, unverified: JP could not determine which account \
                 this credential belongs to, so duplicate-account detection is skipped for it. \
                 The profile is usable. Details: {}",
                self.profile,
                error.unwrap_or_else(|| "no account identity in the response".to_owned()),
            )),
        }

        Ok(())
    }
}

impl List {
    #[expect(clippy::unused_self)]
    fn run(&self, store: &CredentialStore, printer: &Printer) -> Output {
        let document = store.load().map_err(|e| store_error(&e))?;
        let now = Utc::now();

        let mut header = Row::new();
        for label in ["Provider", "Profile", "Type", "Account", "State"] {
            header.add_cell(Cell::new(label));
        }

        let mut rows = Vec::new();
        let mut payload = Vec::new();

        for (category, provider, profile, credential) in document.iter() {
            let target = format!("{category}.{provider}");
            let account = credential
                .email
                .as_deref()
                .or(credential.account_id.as_deref());
            let state = credential_state(credential, now);

            let mut row = Row::new();
            row.add_cell(Cell::new(&target));
            row.add_cell(Cell::new(profile));
            row.add_cell(Cell::new(credential.secret.kind()));
            row.add_cell(Cell::new(account.unwrap_or_default()));
            row.add_cell(Cell::new(&state));
            rows.push(row);

            // Keyed by column heading, matching what the table renders.
            payload.push(serde_json::json!({
                "Provider": target,
                "Profile": profile,
                "Type": credential.secret.kind(),
                "Account": account.unwrap_or_default(),
                "State": state,
            }));
        }

        print_table(
            printer,
            header,
            rows,
            false,
            &serde_json::Value::Array(payload),
        );
        Ok(())
    }
}

impl Logout {
    fn run(&self, store: &CredentialStore, printer: &Printer) -> Output {
        let target = self.target;
        let profile = self.profile.clone();

        let removed = store
            .mutate(|document| {
                let profiles = document
                    .profiles(CATEGORY_LLM, &target.store_key())
                    .map(|profiles| profiles.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();

                let profile = match &profile {
                    Some(profile) => profile.clone(),
                    None if profiles.len() == 1 => profiles[0].clone(),
                    None if profiles.is_empty() => {
                        return Err(StoreError::Rejected(format!(
                            "no stored profiles for {target}"
                        )));
                    }
                    None => {
                        return Err(StoreError::Rejected(format!(
                            "multiple profiles stored for {target} ({}); name one with --profile \
                             <name>",
                            profiles.join(", ")
                        )));
                    }
                };

                document
                    .remove_profile(CATEGORY_LLM, &target.store_key(), &profile)
                    .ok_or_else(|| {
                        StoreError::Rejected(format!(
                            "no stored profile {profile:?} for {target}{}",
                            if profiles.is_empty() {
                                String::new()
                            } else {
                                format!(" (stored: {})", profiles.join(", "))
                            }
                        ))
                    })?;

                Ok(profile)
            })
            .map_err(|e| store_error(&e))?;

        printer.println(format!("Removed {target} profile {removed:?}."));
        Ok(())
    }
}

/// Read the setup token from the first line of stdin.
///
/// Tokens are never accepted as process arguments, which leak into shell
/// history and `ps` output.
fn read_setup_token(printer: &Printer, hint: &str) -> Result<String, Error> {
    if io::stdin().is_terminal() {
        printer.eprintln(format!("Paste the setup token and press Enter.\n{hint}"));
    }

    let mut raw = String::new();
    io::stdin()
        .lock()
        .read_line(&mut raw)
        .map_err(|error| Error::from(format!("failed to read setup token from stdin: {error}")))?;

    sanitize_setup_token(&raw, hint).map_err(Error::from)
}

/// Turn a raw line of token input into a usable token.
///
/// The token is sent to the provider as a bearer credential in an HTTP header,
/// so it must survive that trip.
/// ANSI styling from a decorated command run is stripped; input that still
/// cannot be a bearer token is rejected here, naming what is wrong, rather than
/// deep inside the HTTP client where it surfaces as an opaque `builder error`.
///
/// The rejected shapes are the ones a mis-captured pipeline produces: a
/// progress line, a prompt, or a banner picked up instead of the token.
/// The input is never echoed back, since a valid token is a secret.
fn sanitize_setup_token(raw: &str, hint: &str) -> Result<String, String> {
    let hint = format!("JP reads the token from the first line of stdin. {hint}");

    // Interior carriage returns are checked before ANSI stripping, which
    // discards them: a progress line that overwrites itself would otherwise
    // have its segments spliced into one plausible-looking token.
    let line = raw.trim_end_matches(['\n', '\r']);
    if line.contains('\r') {
        return Err(format!(
            "the input is not a setup token: it contains a carriage return, so it looks like a \
             progress line rather than a credential. {hint}"
        ));
    }

    let token = strip_ansi_escapes::strip_str(line).trim().to_owned();

    if token.is_empty() {
        return Err("no setup token provided on stdin".to_owned());
    }

    // Bearer tokens are a single opaque string (RFC 6750): no whitespace, no
    // control characters, no non-ASCII.
    if token.chars().any(char::is_whitespace) {
        return Err(format!(
            "the input is not a setup token: it contains whitespace, so it looks like a prompt or \
             a message rather than a credential. {hint}"
        ));
    }

    if let Some(bad) = token.chars().find(|c| c.is_control() || !c.is_ascii()) {
        return Err(format!(
            "the input is not a setup token: it contains the character {bad:?}, which cannot be \
             sent in an HTTP header. {hint}"
        ));
    }

    Ok(token)
}

/// The display state of a stored credential, for `jp provider auth list`.
///
/// A static `token` credential has no expiry JP can inspect, so it is reported
/// as plain `valid`, never as expired or refreshable.
fn credential_state(credential: &StoredCredential, now: DateTime<Utc>) -> String {
    let mut parts = vec![];

    if credential.needs_relogin {
        parts.push("needs re-login".to_owned());
    } else if let CredentialSecret::Oauth { expires_at, .. } = &credential.secret {
        if *expires_at <= now {
            parts.push("expired".to_owned());
        } else {
            parts.push(format!("valid (expires {expires_at})"));
        }
    }

    for (scope, expires) in &credential.cooldowns {
        if *expires > now {
            parts.push(format!("cooling down until {expires} ({scope})"));
        }
    }

    if credential.account_id.is_none() {
        parts.push(if credential.needs_relogin {
            "unverified".to_owned()
        } else {
            "unverified (usable)".to_owned()
        });
    }

    if parts.is_empty() {
        return "valid".to_owned();
    }

    parts.join(", ")
}

fn store_error(error: &StoreError) -> Error {
    Error::from(error.to_string())
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
