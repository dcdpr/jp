//! Claude Code login lifecycle, compatibility checks, and ACP queries.
//!
//! Authentication is delegated to the bundled runtime with an explicit login
//! directory.
//! JP never reads Claude Code's credential files.

use std::{
    env, fmt, fs, io,
    process::{ExitStatus, Stdio},
    str::{self, Utf8Error},
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_config::model::id::{ModelIdConfig, Name, ProviderId};
use serde::Deserialize;
use serde_json::from_slice;
use tokio::{io::AsyncReadExt as _, process::Command, time::timeout};
use tracing::{debug, warn};

use super::{BETWEEN_TOOLS_THINKING, MODEL_OVERRIDES, model_overrides};
use crate::{
    credential::AccountIdentity,
    error::StreamError,
    model::{ModelDetails, ReasoningDetails},
};

mod cassette;
mod options;
mod process;
mod protocol;
mod rpc;
mod schema;
mod transcript;
mod transport;
mod usage;

use rpc::RpcError;
use schema::{SessionConfigId, SessionConfigSelectOption};
pub(super) use transport::stream;

/// A failure in the Claude Code subscription flow.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The adapter did not confirm a requested session setting.
    #[error("Claude adapter did not apply session setting `{setting}`")]
    SettingNotApplied { setting: SessionConfigId },

    /// Initialization did not establish the required protocol and transports.
    #[error("Claude adapter lacks the required ACP v1/HTTP MCP capabilities")]
    InitializationCapabilities,
    /// Native history cannot be supplied to this adapter.
    #[error("Claude adapter does not support history loading")]
    HistoryLoadingUnsupported,
    /// A successful ACP response did not include the SDK's final outcome.
    #[error("Claude adapter ended without an SDK result")]
    MissingSdkResult,
    /// Tool-using agent requests need JP's execution endpoint.
    #[error("ACP tool execution requires the JP MCP Host")]
    ToolHostRequired,
    /// The native transcript directory cannot be determined safely.
    #[error(
        "Claude native history requires an absolute configuration directory and working directory"
    )]
    NativeDirectory,
    /// Derived transcript storage failed.
    #[error("Claude native transcript I/O failed")]
    NativeIo(#[source] io::Error),
    /// Derived transcript serialization failed.
    #[error("Claude native transcript serialization failed")]
    NativeJson(#[source] serde_json::Error),
    /// The ACP peer rejected a request or the protocol connection failed.
    #[error("Claude ACP protocol error: {0}")]
    Protocol(#[source] RpcError),
    /// A named ACP subscription has no configured login directory.
    #[error(
        "subscription `{name}` has no Claude Code login directory; run `jp provider llm auth \
         login anthropic --name {name}` to sign in"
    )]
    NamedSubscription { name: String },
    /// A manual mapping disagrees with the registered login directory.
    #[error(
        "subscription `{name}` has conflicting login directories; remove \
         providers.llm.anthropic.acp_config_dirs.{name} to use its registered login"
    )]
    ConflictingDirectory { name: String },
    /// A named login directory is not absolute.
    #[error(
        "providers.llm.anthropic.acp_config_dirs.{name} must be an absolute path, got \
         {directory:?}"
    )]
    InvalidConfigDirectory {
        name: String,
        directory: Utf8PathBuf,
    },
    /// The adapter could not be started or its output could not be read.
    #[error(
        "Claude ACP {check} failed; install @agentclientprotocol/claude-agent-acp@0.81.0 with \
         Node.js 22+ and optional dependencies enabled"
    )]
    Io {
        check: Check,
        #[source]
        source: io::Error,
    },
    /// A prerequisite command did not complete within its deadline.
    #[error("Claude ACP {check} timed out")]
    Timeout { check: Check },
    /// A prerequisite command failed.
    #[error(
        "Claude ACP {check} exited with {status}; check `claude-agent-acp --cli auth status \
         --json`"
    )]
    CommandFailed { check: Check, status: ExitStatus },
    /// A command exceeded the bounded diagnostic output size.
    #[error("Claude ACP {check} returned more than 65536 bytes")]
    OutputLimit { check: Check },
    /// A version command returned invalid UTF-8.
    #[error("Claude ACP {check} returned invalid UTF-8")]
    Encoding {
        check: Check,
        #[source]
        source: Utf8Error,
    },
    /// The installed versions have not been qualified together.
    #[error("unsupported Claude ACP {check}: {actual:?}; expected {expected}")]
    UnsupportedVersion {
        check: Check,
        actual: String,
        expected: &'static str,
    },
    /// Authentication output is not the expected JSON representation.
    #[error("Claude Code returned invalid authentication status")]
    AuthStatus(#[source] serde_json::Error),
    /// A login's status check showed it cannot serve subscription requests.
    #[error(
        "{login} cannot serve subscription requests: {reason}; {}",
        remedy(login, reason)
    )]
    LoginRejected { login: Login, reason: Rejection },
    /// Claude Code did not report a Pro or Max plan for a running session.
    #[error(
        "Claude Code did not confirm a Pro or Max plan for this session; check the login with `jp \
         provider llm auth list`"
    )]
    SubscriptionRequired,
    /// Claude Code could not select the requested model.
    #[error("Claude Code could not select model `{model}`: {source}")]
    ModelSelection {
        model: Name,
        #[source]
        source: RpcError,
    },
    /// The runtime reports an unavailable model.
    #[error("Claude Code cannot use model `{model}`: {detail}")]
    ModelUnavailable { model: Name, detail: String },
    /// The runtime rejects the request or its model parameters.
    #[error("Claude Code rejected the request for model `{model}`: {detail}")]
    RequestRejected { model: Name, detail: String },
    /// A classified failure received through an ACP notification.
    #[error(transparent)]
    Stream(Box<StreamError>),
    /// Changing request implementation requires a fresh Host context.
    #[error(
        "subscription flow changed to ACP during an HTTP request; retry using the current \
         conversation"
    )]
    FlowChanged,
}

/// The Claude Code login a status check inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    /// A login directory selected by a named subscription.
    Named(Utf8PathBuf),
    /// The login Claude Code uses when JP selects none, with the inherited
    /// `CLAUDE_CONFIG_DIR` if one is set.
    Inherited(Option<Utf8PathBuf>),
}

impl fmt::Display for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(directory) => write!(f, "the Claude Code login at {directory}"),
            Self::Inherited(Some(directory)) => write!(
                f,
                "Claude Code's inherited login at {directory} (from CLAUDE_CONFIG_DIR)"
            ),
            Self::Inherited(None) => f.write_str("Claude Code's inherited login at ~/.claude"),
        }
    }
}

/// Why a login cannot serve subscription requests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// No account is signed in.
    #[error("it is signed out")]
    SignedOut,
    /// An API key overrides the account, e.g. from `apiKeyHelper`.
    #[error("it authenticates with an API key from `{origin}`")]
    ApiKey { origin: String },
    /// The account is not a Claude.ai account.
    #[error("it is not signed in with a Claude account")]
    NotClaudeAccount,
    /// The account has no Pro or Max plan.
    #[error("its account has no Pro or Max plan")]
    NoPlan,
    /// Requests go to a cloud provider rather than Anthropic.
    #[error("it does not send requests to Anthropic directly")]
    ThirdParty,
}

fn remedy(login: &Login, reason: &Rejection) -> String {
    let fix = match (reason, login) {
        (Rejection::ApiKey { .. }, _) => {
            "remove that API key setting from Claude Code's settings".to_owned()
        }
        (Rejection::ThirdParty, _) => {
            "remove the provider setting from Claude Code's settings".to_owned()
        }
        // The directory may come from an `acp_config_dirs` mapping that no
        // registration names; without `--config-dir`, login would register a
        // different directory and the next query would report a conflict.
        (_, Login::Named(directory)) => format!(
            "sign in again with `jp provider llm auth login anthropic --name <name> --config-dir \
             {directory}`"
        ),
        (_, Login::Inherited(_)) => {
            "sign in with `claude-agent-acp --cli auth login --claudeai`".to_owned()
        }
    };
    match login {
        Login::Named(_) => fix,
        // The inherited login is what an unnamed `subscription` entry selects,
        // which is easy to reach by accident with `--auth sub`.
        Login::Inherited(_) => format!(
            "the `auth` chain named no subscription, so pass `--auth sub:<name>` to use a \
             registered login (`jp provider llm auth list` shows them), or {fix}"
        ),
    }
}

/// A non-inference operation provided by the installed runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Read the ACP adapter version.
    AdapterVersion,
    /// Read the bundled Claude Code version.
    ClaudeVersion,
    /// Read effective authentication without extracting credentials.
    Authentication,
    /// Sign in through the runtime's interactive authentication command.
    Login,
    /// Clear the selected runtime login.
    Logout,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AdapterVersion => "adapter-version check",
            Self::ClaudeVersion => "Claude Code-version check",
            Self::Authentication => "authentication check",
            Self::Login => "login",
            Self::Logout => "logout",
        })
    }
}

impl Check {
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::AdapterVersion => &["--version"],
            Self::ClaudeVersion => &["--cli", "--version"],
            Self::Authentication => &["--cli", "auth", "status", "--json"],
            Self::Login => &["--cli", "auth", "login", "--claudeai"],
            Self::Logout => &["--cli", "auth", "logout"],
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    logged_in: bool,
    auth_method: Option<AuthMethod>,
    api_provider: Option<ApiProvider>,
    subscription_type: Option<Plan>,
    api_key_source: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
enum AuthMethod {
    #[serde(rename = "claude.ai")]
    ClaudeAccount,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
enum ApiProvider {
    #[serde(rename = "firstParty")]
    FirstParty,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
enum Plan {
    #[serde(rename = "pro", alias = "Claude Pro")]
    Pro,
    #[serde(rename = "max", alias = "Claude Max")]
    Max,
    #[serde(other)]
    Other,
}

/// Verify the installed adapter/runtime pair and its active subscription login.
pub(super) async fn inspect(directory: Option<&Utf8Path>) -> Result<(), Error> {
    inspect_versions(directory).await?;
    let login = match directory {
        Some(directory) => Login::Named(directory.to_owned()),
        None => Login::Inherited(env::var("CLAUDE_CONFIG_DIR").ok().map(Utf8PathBuf::from)),
    };
    validate_auth(&run(Check::Authentication, directory).await?, login).map(drop)
}

async fn inspect_versions(directory: Option<&Utf8Path>) -> Result<(), Error> {
    let adapter = run(Check::AdapterVersion, directory).await?;
    let claude = run(Check::ClaudeVersion, directory).await?;
    qualify_versions(&adapter, &claude)
}

pub(super) async fn login(directory: &Utf8Path) -> Result<AccountIdentity, Error> {
    if !directory.is_absolute() {
        return Err(Error::NativeDirectory);
    }
    inspect_versions(Some(directory)).await?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(directory).map_err(Error::NativeIo)?;
    let mut command = process::command(Some(directory));
    command
        .args(Check::Login.args())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    // Interactive login must stay in the terminal's foreground process group.
    let status = command.status().await.map_err(|source| Error::Io {
        check: Check::Login,
        source,
    })?;
    if !status.success() {
        return Err(Error::CommandFailed {
            check: Check::Login,
            status,
        });
    }
    let output = run(Check::Authentication, Some(directory)).await?;
    validate_auth(&output, Login::Named(directory.to_owned()))
}

pub(super) async fn login_status(directory: &Utf8Path) -> Result<Option<AccountIdentity>, Error> {
    if !directory.is_absolute() {
        return Err(Error::NativeDirectory);
    }
    subscription_identity(&run(Check::Authentication, Some(directory)).await?)
}

pub(super) async fn logout(directory: &Utf8Path) -> Result<(), Error> {
    if !directory.is_absolute() {
        return Err(Error::NativeDirectory);
    }
    run(Check::Logout, Some(directory)).await?;
    Ok(())
}

fn qualify_versions(adapter: &[u8], claude: &[u8]) -> Result<(), Error> {
    for (check, output, expected) in [
        (Check::AdapterVersion, adapter, "0.81.0"),
        (Check::ClaudeVersion, claude, "2.1.280"),
    ] {
        let actual = str::from_utf8(output)
            .map_err(|source| Error::Encoding { check, source })?
            .trim();
        let version = if check == Check::ClaudeVersion {
            actual.strip_suffix(" (Claude Code)").unwrap_or(actual)
        } else {
            actual
        };
        if version != expected {
            return Err(Error::UnsupportedVersion {
                check,
                actual: actual.to_owned(),
                expected,
            });
        }
    }
    Ok(())
}

fn validate_auth(output: &[u8], login: Login) -> Result<AccountIdentity, Error> {
    let status: AuthStatus = serde_json::from_slice(output).map_err(Error::AuthStatus)?;
    status
        .identity()
        .map_err(|reason| Error::LoginRejected { login, reason })
}

fn subscription_identity(output: &[u8]) -> Result<Option<AccountIdentity>, Error> {
    let status: AuthStatus = serde_json::from_slice(output).map_err(Error::AuthStatus)?;
    Ok(status.identity().ok())
}

impl AuthStatus {
    /// The subscription account this status describes, or the first reason it
    /// does not describe one.
    fn identity(self) -> Result<AccountIdentity, Rejection> {
        if !self.logged_in {
            return Err(Rejection::SignedOut);
        }
        if let Some(origin) = self.api_key_source {
            return Err(Rejection::ApiKey { origin });
        }
        if !matches!(self.auth_method, Some(AuthMethod::ClaudeAccount)) {
            return Err(Rejection::NotClaudeAccount);
        }
        if !matches!(self.subscription_type, Some(Plan::Pro | Plan::Max)) {
            return Err(Rejection::NoPlan);
        }
        if !matches!(self.api_provider, Some(ApiProvider::FirstParty)) {
            return Err(Rejection::ThirdParty);
        }
        Ok(AccountIdentity {
            // The runtime reports an organization ID, not an account UUID.
            account_id: None,
            email: self.email.filter(|email| !email.is_empty()),
        })
    }
}

/// Describe the selected model without an API-key-authenticated lookup.
/// Availability is determined by Claude Code when it receives the request.
///
/// Claude Code reports no model capabilities, so reasoning support stays
/// unknown unless the override table marks the model as always thinking.
/// Such a model rejects `thinking: disabled`, which is what an unknown model is
/// sent when reasoning is off.
pub(super) fn model_details(name: &Name) -> ModelDetails {
    let mut model = ModelDetails::empty(ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: name.clone(),
    });
    model.subscription = Some(true);
    model.prefill = Some(false);
    model.structured_output = Some(true);

    if let Some(overrides) = model_overrides(name) {
        // An always-on model thinks adaptively, the only mode it has. The
        // effort ladder is not reported, so both upper levels are assumed, as
        // for a model whose support is unknown.
        if overrides.always_on {
            model.reasoning = Some(ReasoningDetails::adaptive(true, true).always_on());
        }
        if overrides.between_tools {
            model.features.push(BETWEEN_TOOLS_THINKING);
        }
    }

    model
}

/// The model families Claude Code serves, each with the name it is given when
/// the adapter offers no list to choose from.
///
/// An API model belongs to the first family its name contains.
/// A family Anthropic adds later needs an entry here.
const FAMILIES: &[(&str, &str)] = &[
    ("opus", "opus[1m]"),
    ("fable", "claude-fable-5-1[1m]"),
    ("sonnet", "sonnet"),
    ("haiku", "haiku"),
];

/// The family `name` belongs to, and that family's fallback name.
fn family_of(name: &str) -> Option<(&'static str, &'static str)> {
    let name = name.to_ascii_lowercase();
    FAMILIES
        .iter()
        .find(|(family, _)| name.contains(family))
        .copied()
}

/// The name Claude Code is given for the API model `name`, chosen from the
/// values its `model` setting `offers`.
///
/// Conversations and config name models by their API ID on both flows; only the
/// ACP transport speaks Claude Code's names.
/// The model is matched by family rather than version, so a conversation on
/// `claude-opus-5` keeps working after Claude Code moves to a newer Opus:
///
/// - One offered value in the family is that value.
/// - Several are narrowed to one, with a warning, since nothing the user
///   configures can settle it: the 1M-context variant first, then the shortest
///   value (a bare alias, which Claude Code points at its current model, over a
///   pinned version), then the alphabetically first.
/// - With none offered, the family's fallback name is sent.
/// - A model in no known family, or in a family the list does not offer, is
///   sent by its API name, which Claude Code refuses if it does not serve it.
fn subscription_model(name: &str, offers: &[SessionConfigSelectOption]) -> String {
    let Some((family, fallback)) = family_of(name) else {
        return name.to_owned();
    };
    if offers.is_empty() {
        return fallback.to_owned();
    }

    let mut matches: Vec<&str> = offers
        .iter()
        .map(|option| option.value.0.as_str())
        .filter(|value| value.to_ascii_lowercase().contains(family))
        .collect();
    matches.sort_by_key(|value| (!value.ends_with("[1m]"), value.len(), *value));

    match matches.as_slice() {
        [] => {
            warn!(
                model = name,
                family, "Claude Code offers no model in this family."
            );
            name.to_owned()
        }
        [only] => (*only).to_owned(),
        [first, ..] => {
            warn!(
                model = name,
                family,
                candidates = ?matches,
                chosen = first,
                "Claude Code offers several models in this family; using the first."
            );
            (*first).to_owned()
        }
    }
}

/// The API model JP records for a value Claude Code offers, labelled
/// `display_name`.
///
/// Tried in order, each against JP's model catalog:
///
/// - The value names an API model (`claude-fable-5-1[1m]`).
/// - The label names one: `Sonnet 5` is `claude-sonnet-5`.
///   A bare alias says nothing about its version, and the label is what does.
/// - The newest cataloged model in the value's family.
///
/// `None` for a value in no known family, such as `default`.
fn api_name(value: &str, display_name: Option<&str>) -> Option<&'static str> {
    let (family, _) = family_of(value)?;
    let base = value.split_once('[').map_or(value, |(base, _)| base);
    let labelled = display_name.map(|name| {
        format!(
            "claude-{}",
            name.trim().to_ascii_lowercase().replace([' ', '.'], "-")
        )
    });

    MODEL_OVERRIDES
        .get(base)
        .or_else(|| labelled.as_deref().and_then(|id| MODEL_OVERRIDES.get(id)))
        .filter(|model| model.id.contains(family))
        .or_else(|| {
            MODEL_OVERRIDES
                .values()
                .find(|model| model.id.contains(family))
        })
        .map(|model| model.id)
}

/// The models Claude Code offers the selected login, by API model ID.
///
/// Read from the adapter's `model` session setting.
pub(super) async fn models(directory: Option<&Utf8Path>) -> Result<Vec<ModelDetails>, Error> {
    let options = transport::list_models(directory).await?;
    Ok(served_models(&options))
}

/// The listed models JP can name, once each.
///
/// A value in no known family is left out: picking it would record a name that
/// only the ACP flow understands.
/// That includes `default`, which stands for whichever model Claude Code
/// recommends and is listed again under its own name.
fn served_models(options: &[SessionConfigSelectOption]) -> Vec<ModelDetails> {
    let mut models: Vec<ModelDetails> = vec![];
    for option in options {
        let Some(api) = api_name(&option.value.0, option.name.as_deref()) else {
            debug!(value = %option.value, "Skipping a Claude Code model with no API name.");
            continue;
        };
        if models.iter().any(|model| model.id.name.as_ref() == api) {
            continue;
        }
        let mut model = model_details(&Name(api.to_owned()));
        model.display_name.clone_from(&option.name);
        models.push(model);
    }
    models
}

fn removes_variable(name: &str) -> bool {
    let normalized = name.to_ascii_uppercase();
    let name = normalized.as_str();
    name.starts_with("ANTHROPIC_")
        || name.starts_with("CLAUDE_CODE_USE_")
        || name.starts_with("DISABLE_PROMPT_CACHING")
        || name == "CLAUDE_CODE_PROMPT_CACHE_TTL"
        || matches!(
            name,
            "CLAUDE_CODE_OAUTH_TOKEN"
                | "CLAUDE_CODE_API_KEY"
                | "CLAUDECODE"
                | "FORCE_PROMPT_CACHING_5M"
                | "ENABLE_PROMPT_CACHING_1H"
        )
}

async fn run(check: Check, directory: Option<&Utf8Path>) -> Result<Vec<u8>, Error> {
    let mut command = process::command(directory);
    command.args(check.args());
    read_output(command, check).await
}

async fn read_output(mut command: Command, check: Check) -> Result<Vec<u8>, Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = process::spawn(command).map_err(|source| Error::Io { check, source })?;
    let result = timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        child
            .stdout()
            .take()
            .expect("stdout is piped")
            .take(65_537)
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| Error::Io { check, source })?;
        if bytes.len() > 65_536 {
            return Err(Error::OutputLimit { check });
        }
        let status = child
            .wait()
            .await
            .map_err(|source| Error::Io { check, source })?;
        // A signed-out runtime returns JSON status with exit code 1.
        let signed_out = matches!(check, Check::Authentication)
            && status.code() == Some(1)
            && from_slice::<AuthStatus>(&bytes).is_ok_and(|status| !status.logged_in);
        if !(status.success() || signed_out) {
            return Err(Error::CommandFailed { check, status });
        }
        Ok(bytes)
    })
    .await;
    let result = result.unwrap_or(Err(Error::Timeout { check }));
    if result.is_err()
        && child.id().is_some()
        && let Err(error) = Box::into_pin(child.kill()).await
    {
        warn!(%error, %check, "Failed to stop Claude ACP inspection process.");
    }
    result
}

#[cfg(test)]
#[path = "acp/recorded_tests.rs"]
mod recorded_tests;

#[cfg(test)]
#[path = "acp/workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
