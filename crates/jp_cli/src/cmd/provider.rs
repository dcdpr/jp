//! Provider credential management: `jp provider llm auth login|list|logout`.
//!
//! The provider category sits between `provider` and `auth`, so a provider is
//! named plainly (`anthropic`) and a future category can offer different
//! commands.
//!
//! Credentials are user-global, so these commands bypass workspace discovery
//! entirely, the same startup exception `jp init` uses.
//! `list` and `logout` require no TTY; `login` reads its setup token from
//! stdin, never from process arguments.

use std::{
    collections::HashMap,
    env, fmt,
    io::{self, BufRead as _, IsTerminal as _},
    net::Ipv4Addr,
    str::FromStr,
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use comfy_table::{Cell, Row};
use crossterm::style::{Color, Stylize as _};
use jp_config::{
    FillDefaults as _, PartialAppConfig, PartialConfig as _, model::id::ProviderId,
    types::api_key_env::ApiKeyEnv,
};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, StoreError, StoredCredential,
};
use jp_llm::{
    credential::{AccountIdentity, ProviderAuth},
    provider::openai::{auth as openai_auth, oauth as openai_oauth},
};
use jp_printer::Printer;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

/// How long a login waits for the user to finish signing in.
const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);

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
    /// Model providers: the services that answer a query.
    Llm(Llm),
}

/// `jp provider llm` subcommand group.
#[derive(Debug, clap::Args)]
struct Llm {
    #[command(subcommand)]
    command: LlmCmd,
}

#[derive(Debug, clap::Subcommand)]
enum LlmCmd {
    /// Manage the credentials a model provider is reached with.
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
    /// The provider to log in to, e.g. `anthropic`.
    target: AuthTarget,

    /// The name to store the credential under, selected from an `auth` chain as
    /// `subscription:<name>`.
    #[arg(long, default_value = "default")]
    name: String,

    /// Sign in by entering a code on the provider's website, instead of
    /// capturing a browser redirect on localhost.
    ///
    /// Use this on a machine whose browser cannot reach back to it, or where
    /// the callback port is unavailable.
    #[arg(long, conflicts_with_all = ["setup_token", "import_codex"])]
    device_auth: bool,

    /// Copy the credential the Codex CLI already holds, instead of signing in.
    ///
    /// Reads `$CODEX_HOME/auth.json` (`~/.codex/auth.json` by default).
    /// The credential is copied, not moved: refreshing it through JP rotates
    /// the refresh token, which ends the Codex CLI's own session.
    #[arg(long, conflicts_with_all = ["setup_token", "device_auth"])]
    import_codex: bool,

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
    /// The provider to log out of, e.g. `anthropic`.
    target: AuthTarget,

    /// The name of the credential to remove.
    ///
    /// When omitted, removes the sole stored credential.
    #[arg(long)]
    name: Option<String>,
}

/// A model provider that supports stored credentials.
///
/// Parses only the providers [`jp_llm::provider_auth`] reports mechanics for;
/// an API-key-only provider is rejected.
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
        self.provider.fmt(f)
    }
}

impl FromStr for AuthTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let provider: ProviderId = s
            .parse()
            .map_err(|_| format!("unknown model provider {s:?}"))?;

        if jp_llm::provider_auth(provider).is_none() {
            return Err(format!(
                "`{s}` has no stored credentials: it authenticates with an API key, set through \
                 `providers.llm.{s}.api_key_env`"
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
        let ProviderCmd::Llm(llm) = &self.command;
        let LlmCmd::Auth(auth) = &llm.command;
        let store = CredentialStore::file_default().map_err(|e| store_error(&e))?;

        match &auth.command {
            AuthCmd::Login(args) => {
                let runtime = crate::build_runtime(None, "jp-provider-auth")
                    .map_err(crate::cmd::Error::from)?;
                runtime.block_on(args.run(&store, printer))
            }
            // Read here rather than in `run`, so a test supplies its own keys
            // instead of reading the machine's config.
            AuthCmd::List(args) => args.run(&store, &configured_api_keys(printer), printer),
            AuthCmd::Logout(args) => args.run(&store, printer),
        }
    }
}

/// The credential a login produced.
enum Acquired {
    /// A static bearer token with no refresh flow.
    Token(String),

    /// A refreshable token pair.
    Oauth {
        access_token: String,
        refresh_token: String,
        expires_at: DateTime<Utc>,
        identity: AccountIdentity,
    },
}

impl Login {
    async fn run(&self, store: &CredentialStore, printer: &Printer) -> Output {
        let auth = jp_llm::provider_auth(self.target.provider)
            .expect("AuthTarget parsing guarantees stored-credential support");

        if self.name.is_empty() || self.name.chars().any(char::is_whitespace) {
            return Err(Error::from(format!(
                "invalid credential name {:?}: must be non-empty and contain no whitespace",
                self.name
            )));
        }

        let acquired = self.acquire(auth.as_ref(), printer).await?;

        // Best-effort identity recovery: a failure stores the profile
        // unverified instead of blocking the login.
        let (secret, identity) = match acquired {
            Acquired::Token(token) => {
                let identity = auth.recover_identity(&token).await;
                (CredentialSecret::Token { token }, identity)
            }
            Acquired::Oauth {
                access_token,
                refresh_token,
                expires_at,
                identity,
            } => (
                CredentialSecret::Oauth {
                    access_token,
                    refresh_token,
                    expires_at,
                },
                Ok(identity),
            ),
        };

        let profile = self.name.clone();
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
                        "account {account_id} is already stored as {existing:?}; run `jp provider \
                         auth logout {target} --name {existing}` first, or use that credential"
                    )));
                }

                document.insert_profile(
                    CATEGORY_LLM,
                    &target.store_key(),
                    &profile,
                    StoredCredential {
                        secret: secret.clone(),
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
                "Linked {target} credential {:?} to {} (account {account_id}).",
                self.name,
                email.as_deref().unwrap_or("<no email>"),
            )),
            // An unattributable credential is a normal outcome, not a
            // misconfiguration: a credential whose scopes don't permit an
            // identity lookup can still authenticate requests.
            (None, error) => printer.println(format!(
                "Stored {target} credential {:?}, unverified: JP could not determine which \
                 account it belongs to, so duplicate-account detection is skipped for it. The \
                 credential is usable. Details: {}",
                self.name,
                error.unwrap_or_else(|| "no account identity in the response".to_owned()),
            )),
        }

        Ok(())
    }
}

impl Login {
    /// Obtain a credential by whichever flow the flags selected.
    async fn acquire(&self, auth: &dyn ProviderAuth, printer: &Printer) -> Result<Acquired, Error> {
        if self.setup_token {
            return Ok(Acquired::Token(read_setup_token(
                printer,
                auth.setup_token_hint(),
            )?));
        }

        if self.import_codex {
            return self.import_codex();
        }

        if self.target.provider != ProviderId::Openai {
            return Err(Error::from(format!(
                "browser login is not implemented for {}; pass a setup token on stdin with \
                 `--setup-token`. {}",
                self.target,
                auth.setup_token_hint()
            )));
        }

        if self.device_auth {
            return self.device_login(printer).await;
        }

        self.browser_login(printer).await
    }

    /// Copy the credential the Codex CLI already holds.
    fn import_codex(&self) -> Result<Acquired, Error> {
        if self.target.provider != ProviderId::Openai {
            return Err(Error::from(format!(
                "--import-codex reads the Codex CLI's credential, which only applies to \
                 llm.openai, not {}",
                self.target
            )));
        }

        let path = openai_auth::codex_auth_path().ok_or_else(|| {
            Error::from(
                "could not determine the Codex CLI's home directory; set CODEX_HOME".to_owned(),
            )
        })?;

        let imported = openai_auth::import_codex_credential(&path)
            .map_err(|error| Error::from(error_chain(&error)))?;

        Ok(Acquired::Oauth {
            access_token: imported.access_token,
            refresh_token: imported.refresh_token,
            expires_at: imported.expires_at,
            identity: imported.identity,
        })
    }

    /// Sign in by having the user enter a code on the provider's site.
    ///
    /// Needs no reachable callback, which is what makes it work over SSH and in
    /// containers.
    async fn device_login(&self, printer: &Printer) -> Result<Acquired, Error> {
        let device = openai_oauth::start_device_auth()
            .await
            .map_err(|error| Error::from(error_chain(&error)))?;

        printer.eprintln(format!(
            "Open {} and enter the code: {}",
            openai_oauth::device_verification_url(),
            device.user_code
        ));

        let deadline = Instant::now() + LOGIN_TIMEOUT;

        loop {
            if Instant::now() >= deadline {
                return Err(Error::from(
                    "device authorization timed out; run the login again".to_owned(),
                ));
            }

            match openai_oauth::poll_device_auth(&device).await {
                Ok(openai_oauth::DevicePoll::Ready(tokens)) => return Ok(tokens.into()),
                Ok(openai_oauth::DevicePoll::Pending) => {
                    tokio::time::sleep(device.poll_interval()).await;
                }
                Err(error) => return Err(Error::from(error_chain(&error))),
            }
        }
    }

    /// Sign in through the browser, capturing the redirect on localhost.
    async fn browser_login(&self, printer: &Printer) -> Result<Acquired, Error> {
        let pkce = openai_oauth::Pkce::generate();
        let state = openai_oauth::random_token();
        let redirect_uri = openai_oauth::callback_redirect_uri();
        let url = openai_oauth::authorize_url(&pkce.challenge, &state, &redirect_uri);

        // Bound to the loopback interface: the redirect carries an
        // authorization code, and nothing off this machine should see it.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, openai_oauth::CALLBACK_PORT))
            .await
            .map_err(|error| {
                Error::from(format!(
                    "could not listen on localhost:{} for the login redirect: {error}. The \
                     provider only accepts that exact port, so free it or use `--device-auth`.",
                    openai_oauth::CALLBACK_PORT
                ))
            })?;

        printer.eprintln(format!("Open this URL to sign in:\n{url}"));

        let code = tokio::time::timeout(LOGIN_TIMEOUT, await_callback(&listener, &state))
            .await
            .map_err(|_| Error::from("login timed out waiting for the redirect".to_owned()))?
            .map_err(Error::from)?;

        let tokens = openai_oauth::exchange_code(&code, &pkce.verifier, &redirect_uri)
            .await
            .map_err(|error| Error::from(error_chain(&error)))?;

        Ok(tokens.into())
    }
}

impl From<openai_oauth::Tokens> for Acquired {
    fn from(tokens: openai_oauth::Tokens) -> Self {
        Self::Oauth {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            identity: tokens.identity,
        }
    }
}

/// Serve the OAuth redirect until it arrives, and return its `code`.
///
/// Connections that are not the redirect are answered and dropped: a browser
/// preflight or a stray request must not end the login.
async fn await_callback(listener: &TcpListener, state: &str) -> Result<String, String> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|error| format!("login redirect connection failed: {error}"))?;

        let mut buffer = [0u8; 2048];
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();

        let Some(query) = request_query(&request) else {
            respond(&mut socket, "Not found").await;
            continue;
        };

        let params = parse_query(&query);

        if let Some(error) = params.get("error_description").or(params.get("error")) {
            respond(&mut socket, "Sign-in failed. Return to your terminal.").await;
            return Err(format!("the provider refused the sign-in: {error}"));
        }

        // The state ties the redirect to the request JP started; a mismatch
        // means this redirect belongs to someone else's login attempt.
        if params.get("state").map(String::as_str) != Some(state) {
            respond(&mut socket, "Sign-in failed. Return to your terminal.").await;
            return Err("the login redirect carried an unexpected state value".to_owned());
        }

        let Some(code) = params.get("code") else {
            respond(&mut socket, "Sign-in failed. Return to your terminal.").await;
            return Err("the login redirect carried no authorization code".to_owned());
        };

        respond(&mut socket, "Signed in. Return to your terminal.").await;

        return Ok(code.clone());
    }
}

/// The query string of a request for the OAuth callback path.
fn request_query(request: &str) -> Option<String> {
    let target = request.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;

    (path == openai_oauth::CALLBACK_PATH).then(|| query.to_owned())
}

/// Decode a `key=value&…` query string.
fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), percent_decode(value)))
        .collect()
}

/// Decode percent-escapes and `+` in a query parameter value.
fn percent_decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).to_string()
}

/// Answer the browser so the user sees something other than a dead tab.
async fn respond(socket: &mut TcpStream, message: &str) {
    let body = format!("<!doctype html><html><body><p>{message}</p></body></html>");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \
         {}\r\n\r\n{body}",
        body.len()
    );

    drop(socket.write_all(response.as_bytes()).await);
    drop(socket.flush().await);
}

impl List {
    #[expect(clippy::unused_self)]
    fn run(
        &self,
        store: &CredentialStore,
        api_keys: &[(String, String, String)],
        printer: &Printer,
    ) -> Output {
        let document = store.load().map_err(|e| store_error(&e))?;
        let now = Utc::now();

        let mut header = Row::new();
        for label in ["Provider", "Name", "Kind", "State"] {
            header.add_cell(Cell::new(label));
        }

        let mut rows = Vec::new();
        let mut payload = Vec::new();

        // Keys before subscriptions, so the table reads in the order a default
        // `auth` chain resolves.
        for (target, name, variable) in api_keys {
            let state = KeyState::read(variable);
            let prose = state.to_prose(variable);

            let mut row = Row::new();
            row.add_cell(Cell::new(target));
            row.add_cell(Cell::new(name));
            row.add_cell(Cell::new("api_key"));
            row.add_cell(Cell::new(match state.fault() {
                Some(fault) => emphasize(&prose, fault),
                None => prose,
            }));
            rows.push(row);

            payload.push(serde_json::json!({
                "provider": target,
                "name": name,
                "kind": "api_key",
                "env": variable,
                "state": state.as_str(),
            }));
        }

        // The store is keyed by category and provider; the category is fixed by
        // the command path.
        for (_, provider, profile, credential) in document.iter() {
            let target = provider;
            let state = CredentialState::read(credential, now);
            let prose = state.to_prose();

            let mut row = Row::new();
            row.add_cell(Cell::new(target));
            row.add_cell(Cell::new(profile));
            // A stored credential is always a subscription; `secret.kind()` is
            // the mechanism it arrived by, which no `auth` entry selects on.
            row.add_cell(Cell::new("subscription"));
            row.add_cell(Cell::new(highlight_faults(&prose)));
            rows.push(row);

            let mut entry = serde_json::json!({
                "provider": target,
                "name": profile,
                "kind": "subscription",
                "mechanism": credential.secret.kind(),
            });

            if let (Some(entry), Some(state)) = (entry.as_object_mut(), state.to_json().as_object())
            {
                entry.extend(state.clone());
            }

            payload.push(entry);
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

/// Every API key the configuration names, as `(provider, name, variable)`.
///
/// An unreadable configuration yields nothing and reports why on stderr, so the
/// stored credentials still list.
fn configured_api_keys(printer: &Printer) -> Vec<(String, String, String)> {
    match read_api_keys() {
        Ok(keys) => keys,
        Err(error) => {
            printer.eprintln(format!(
                "could not read the configured API keys, so only stored credentials are listed: \
                 {error}"
            ));
            vec![]
        }
    }
}

/// Read `api_key_env` for every model provider that has one.
///
/// Reads the user-global config and the `.jp.toml` chain, but not a workspace's
/// own config: the command runs before workspace discovery, since credentials
/// are user-global.
fn read_api_keys() -> Result<Vec<(String, String, String)>, crate::Error> {
    let cwd = env::current_dir().map_err(|error| {
        crate::Error::CliConfig(format!("cannot read the current directory: {error}"))
    })?;
    let cwd = Utf8PathBuf::from_path_buf(cwd).map_err(|path| {
        crate::Error::CliConfig(format!("path is not UTF-8: {}", path.display()))
    })?;

    let partial = crate::load_base_partial(None, cwd)?;
    let defaults = PartialAppConfig::default_values(&())
        .map_err(|error| crate::Error::CliConfig(error.to_string()))?
        .unwrap_or_default();
    let config = partial.fill_from(defaults).providers.llm;

    // `llamacpp` and `ollama` are absent: they are reached over a local socket
    // and have no key to name.
    let keys = [
        (ProviderId::Anthropic, config.anthropic.api_key_env),
        (ProviderId::Cerebras, config.cerebras.api_key_env),
        (ProviderId::Deepseek, config.deepseek.api_key_env),
        (ProviderId::Google, config.google.api_key_env),
        (ProviderId::Openai, config.openai.api_key_env),
        (ProviderId::Openrouter, config.openrouter.api_key_env),
    ];

    Ok(keys
        .into_iter()
        .filter_map(|(provider, env)| Some((provider, env?)))
        .flat_map(|(provider, env)| {
            let target = provider.to_string();

            match env {
                // Named for the chain entry that selects it.
                ApiKeyEnv::One(variable) => vec![(target, "api_key".to_owned(), variable)],
                ApiKeyEnv::Many(variables) => variables
                    .into_iter()
                    .map(|(name, variable)| (target.clone(), name, variable))
                    .collect(),
            }
        })
        .collect())
}

impl Logout {
    fn run(&self, store: &CredentialStore, printer: &Printer) -> Output {
        let target = self.target;
        let profile = self.name.clone();

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
                            "multiple credentials stored for {target} ({}); name one with --name \
                             <name>",
                            profiles.join(", ")
                        )));
                    }
                };

                document
                    .remove_profile(CATEGORY_LLM, &target.store_key(), &profile)
                    .ok_or_else(|| {
                        StoreError::Rejected(format!(
                            "no stored credential {profile:?} for {target}{}",
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

/// The words in a rendered [`CredentialState`] that mean it cannot serve a
/// request.
///
/// A cooldown is not among them: it clears on its own.
const FAULTS: &[&str] = &["needs re-login", "expired"];

/// Style every [`FAULTS`] word a rendered state contains.
fn highlight_faults(state: &str) -> String {
    FAULTS
        .iter()
        .fold(state.to_owned(), |state, fault| emphasize(&state, fault))
}

/// Style the first occurrence of `needle` within `text` as a fault.
fn emphasize(text: &str, needle: &str) -> String {
    if !text.contains(needle) {
        return text.to_owned();
    }

    text.replace(
        needle,
        &needle.to_owned().bold().with(Color::Red).to_string(),
    )
}

/// A number of seconds, in words, rounded to whole units.
///
/// The sign is dropped: a caller supplies the direction.
fn humanize(seconds: i64) -> String {
    let seconds = u64::try_from(seconds.abs()).unwrap_or_default();

    // Whole units only: a trailing "3m 12s 400ms" reads as noise next to the
    // "6 days" it is attached to.
    let rounded = match seconds {
        0..60 => seconds,
        60..3600 => seconds / 60 * 60,
        _ => seconds / 3600 * 3600,
    };

    humantime::format_duration(Duration::from_secs(rounded)).to_string()
}

/// Whether an environment variable holds an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyState {
    /// The variable holds a key.
    Ok,

    /// The variable is not in the environment.
    Unset,

    /// The variable is in the environment, holding nothing.
    Empty,
}

impl KeyState {
    /// Read the variable from the environment.
    ///
    /// A whitespace-only value counts as [`Self::Empty`].
    fn read(variable: &str) -> Self {
        match env::var(variable) {
            Ok(value) if value.trim().is_empty() => Self::Empty,
            Ok(_) => Self::Ok,
            Err(_) => Self::Unset,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unset => "unset",
            Self::Empty => "empty",
        }
    }

    /// The word in [`Self::to_prose`] worth marking, if the state needs
    /// attention.
    const fn fault(self) -> Option<&'static str> {
        match self {
            Self::Ok => None,
            Self::Unset => Some("not set"),
            Self::Empty => Some("empty"),
        }
    }

    fn to_prose(self, variable: &str) -> String {
        match self {
            Self::Ok => format!("{variable} is set"),
            Self::Unset => format!("{variable} is not set"),
            Self::Empty => format!("{variable} is empty"),
        }
    }
}

/// The state of a stored credential, rendered by [`CredentialState::to_prose`]
/// for the table and [`CredentialState::to_json`] for the payload.
struct CredentialState {
    lifecycle: Lifecycle,

    /// Whether JP knows which account the credential belongs to.
    ///
    /// An unverified credential still authenticates; it takes no part in
    /// duplicate-account detection.
    verified: bool,

    /// Seconds until the access token expires, or `None` for a mechanism with
    /// no expiry JP can read.
    expires_in_secs: Option<i64>,

    /// Quota cooldowns still in effect, as `(scope, seconds remaining)`.
    cooldowns: Vec<(String, i64)>,
}

/// Whether a stored credential can serve a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    /// Usable, subject to any cooldowns.
    Valid,

    /// The access token's expiry has passed; a refresh is due.
    Expired,

    /// The provider refused the credential, or a refresh was rejected.
    NeedsRelogin,
}

impl Lifecycle {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Expired => "expired",
            Self::NeedsRelogin => "needs_relogin",
        }
    }
}

impl CredentialState {
    /// Read the state of a stored credential.
    ///
    /// A `token` credential carries no expiry, so it never reads as expired.
    /// Cooldowns already elapsed are dropped.
    fn read(credential: &StoredCredential, now: DateTime<Utc>) -> Self {
        let expiry = match &credential.secret {
            CredentialSecret::Oauth { expires_at, .. } => Some(*expires_at),
            CredentialSecret::Token { .. } => None,
        };

        let lifecycle = if credential.needs_relogin {
            Lifecycle::NeedsRelogin
        } else if expiry.is_some_and(|expires_at| expires_at <= now) {
            Lifecycle::Expired
        } else {
            Lifecycle::Valid
        };

        Self {
            lifecycle,
            verified: credential.account_id.is_some(),
            expires_in_secs: expiry.map(|expires_at| (expires_at - now).num_seconds()),
            cooldowns: credential
                .cooldowns
                .iter()
                .filter(|(_, expires)| **expires > now)
                .map(|(scope, expires)| (scope.clone(), (*expires - now).num_seconds()))
                .collect(),
        }
    }

    fn to_prose(&self) -> String {
        let mut parts = vec![];

        match self.lifecycle {
            Lifecycle::NeedsRelogin => parts.push("needs re-login".to_owned()),
            Lifecycle::Expired => parts.push("expired".to_owned()),
            Lifecycle::Valid => {
                if let Some(secs) = self.expires_in_secs {
                    parts.push(format!("valid (expires in {})", humanize(secs)));
                }
            }
        }

        for (scope, secs) in &self.cooldowns {
            parts.push(format!("cooling down for {} ({scope})", humanize(*secs)));
        }

        if !self.verified {
            parts.push(match self.lifecycle {
                Lifecycle::NeedsRelogin => "unverified".to_owned(),
                _ => "unverified (usable)".to_owned(),
            });
        }

        if parts.is_empty() {
            return "valid".to_owned();
        }

        parts.join(", ")
    }

    fn to_json(&self) -> Value {
        serde_json::json!({
            "state": self.lifecycle.as_str(),
            "verified": self.verified,
            "expires_in_secs": self.expires_in_secs,
            "cooldowns": self
                .cooldowns
                .iter()
                .map(|(scope, secs)| serde_json::json!({
                    "scope": scope,
                    "expires_in_secs": secs,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

fn store_error(error: &StoreError) -> Error {
    Error::from(error.to_string())
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
