//! Claude Code login lifecycle, compatibility checks, and ACP queries.
//!
//! Authentication is delegated to the bundled runtime with an explicit login
//! directory.
//! JP never reads Claude Code's credential files.

use std::{
    fmt, fs, io,
    process::{ExitStatus, Stdio},
    str::{self, Utf8Error},
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_config::model::id::{ModelIdConfig, Name, ProviderId};
use serde::Deserialize;
use serde_json::from_slice;
use tokio::{io::AsyncReadExt as _, process::Command, time::timeout};
use tracing::warn;

use crate::{credential::AccountIdentity, error::StreamError, model::ModelDetails};

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
use schema::SessionConfigId;
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
    /// Effective subscription authentication could not be established.
    #[error(
        "Claude Code must use an active Pro or Max subscription login; run `claude-agent-acp \
         --cli auth login --claudeai` and remove conflicting API-key/helper or cloud \
         configuration; disable paid Usage credits to prevent overage"
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
    validate_auth(&run(Check::Authentication, directory).await?)
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
    login_status(directory)
        .await?
        .ok_or(Error::SubscriptionRequired)
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

fn validate_auth(output: &[u8]) -> Result<(), Error> {
    subscription_identity(output)?
        .map(drop)
        .ok_or(Error::SubscriptionRequired)
}

fn subscription_identity(output: &[u8]) -> Result<Option<AccountIdentity>, Error> {
    let status: AuthStatus = serde_json::from_slice(output).map_err(Error::AuthStatus)?;
    if status.logged_in
        && status.api_key_source.is_none()
        && matches!(status.auth_method, Some(AuthMethod::ClaudeAccount))
        && matches!(status.api_provider, Some(ApiProvider::FirstParty))
        && matches!(status.subscription_type, Some(Plan::Pro | Plan::Max))
    {
        return Ok(Some(AccountIdentity {
            // The runtime reports an organization ID, not an account UUID.
            account_id: None,
            email: status.email.filter(|email| !email.is_empty()),
        }));
    }
    Ok(None)
}

/// Describe the selected model without an API-key-authenticated lookup.
/// Availability is determined by Claude Code when it receives the request.
pub(super) fn model_details(name: &Name) -> ModelDetails {
    let mut model = ModelDetails::empty(ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: name.clone(),
    });
    model.subscription = Some(true);
    model.prefill = Some(false);
    model.structured_output = Some(true);
    model
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
