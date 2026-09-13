//! Compatibility checks for the Claude Code subscription flow.
//!
//! Inspection invokes version and authentication commands only.
//! No prompt is submitted and JP never reads Claude Code's credential files.

use std::{
    env, fmt, io,
    process::{ExitStatus, Stdio},
    str::{self, Utf8Error},
    time::Duration,
};

use agent_client_protocol::{Error as RpcError, schema::v1::SessionConfigId};
use jp_config::model::id::{ModelIdConfig, Name, ProviderId};
use serde::Deserialize;
use tokio::{io::AsyncReadExt as _, process::Command, time::timeout};
use tracing::warn;

use crate::model::ModelDetails;

mod options;
mod protocol;
mod transcript;
mod transport;
pub(super) use transport::stream;

/// A failed prerequisite for the Claude Code subscription flow.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The adapter did not confirm a requested session setting.
    #[error("Claude adapter did not apply session setting `{setting}`")]
    SettingNotApplied { setting: SessionConfigId },
    /// An explicitly configured parameter has no qualified ACP mapping.
    #[error("assistant.model.parameters.{parameter} is not supported by the ACP subscription flow")]
    UnsupportedParameter { parameter: String },

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
    /// The process-group lifecycle has not been qualified on this platform.
    #[error("the ACP subscription flow currently requires a Unix host")]
    PlatformUnsupported,
    /// The native transcript directory cannot be determined safely.
    #[error(
        "Claude native history requires an absolute HOME/CLAUDE_CONFIG_DIR and a working \
         directory whose encoded name is at most 200 bytes"
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
    /// JP names do not select Claude Code accounts.
    #[error(
        "subscription credential `{name}` is not mapped to a Claude Code login; use an unnamed \
         `subscription` entry, or explicitly set providers.llm.anthropic.subscription_flow=direct \
         to use JP-stored credentials (account-policy risk)"
    )]
    NamedSubscription { name: String },
    /// The adapter could not be started or its output could not be read.
    #[error(
        "Claude ACP {check} failed; install @agentclientprotocol/claude-agent-acp@0.76.0 with \
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
    /// The selected model has not been qualified for this flow.
    #[error(
        "model `{model}` is not qualified for the ACP subscription flow; the qualified model is \
         claude-opus-5"
    )]
    UnsupportedModel { model: Name },
    /// Changing request implementation requires a fresh Host context.
    #[error(
        "subscription flow changed to ACP during an HTTP request; retry using the current \
         conversation"
    )]
    FlowChanged,
}

/// A non-inference operation used to inspect the installed runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Read the ACP adapter version.
    AdapterVersion,
    /// Read the bundled Claude Code version.
    ClaudeVersion,
    /// Read effective authentication without extracting credentials.
    Authentication,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AdapterVersion => "adapter-version check",
            Self::ClaudeVersion => "Claude Code-version check",
            Self::Authentication => "authentication check",
        })
    }
}

impl Check {
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::AdapterVersion => &["--version"],
            Self::ClaudeVersion => &["--cli", "--version"],
            Self::Authentication => &["--cli", "auth", "status", "--json"],
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
pub(super) async fn inspect() -> Result<(), Error> {
    let adapter = run(Check::AdapterVersion).await?;
    let claude = run(Check::ClaudeVersion).await?;
    qualify_versions(&adapter, &claude)?;
    validate_auth(&run(Check::Authentication).await?)
}

fn qualify_versions(adapter: &[u8], claude: &[u8]) -> Result<(), Error> {
    for (check, output, expected) in [
        (Check::AdapterVersion, adapter, "0.76.0"),
        (Check::ClaudeVersion, claude, "2.1.257"),
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
    let status: AuthStatus = serde_json::from_slice(output).map_err(Error::AuthStatus)?;
    if status.logged_in
        && status.api_key_source.is_none()
        && matches!(status.auth_method, Some(AuthMethod::ClaudeAccount))
        && matches!(status.api_provider, Some(ApiProvider::FirstParty))
        && matches!(status.subscription_type, Some(Plan::Pro | Plan::Max))
    {
        return Ok(());
    }
    Err(Error::SubscriptionRequired)
}

/// Qualified model metadata without making an API-key-authenticated request.
pub(super) fn model_details(name: &Name) -> Result<ModelDetails, Error> {
    if name.as_ref() != "claude-opus-5" {
        return Err(Error::UnsupportedModel {
            model: name.clone(),
        });
    }
    let mut model = ModelDetails::empty(ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: name.clone(),
    });
    model.subscription = Some(true);
    model.prefill = Some(false);
    model.structured_output = Some(true);
    Ok(model)
}

fn removes_variable(name: &str) -> bool {
    name.starts_with("ANTHROPIC_")
        || name.starts_with("CLAUDE_CODE_USE_")
        || matches!(
            name,
            "CLAUDE_CODE_OAUTH_TOKEN" | "CLAUDE_CODE_API_KEY" | "CLAUDECODE"
        )
}

async fn run(check: Check) -> Result<Vec<u8>, Error> {
    let mut command = Command::new("claude-agent-acp");
    command.args(check.args());
    for (name, _) in env::vars_os() {
        if name.to_str().is_some_and(removes_variable) {
            command.env_remove(name);
        }
    }
    read_output(&mut command, check).await
}

async fn read_output(command: &mut Command, check: Check) -> Result<Vec<u8>, Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|source| Error::Io { check, source })?;
    let result = timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        child
            .stdout
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
        if !status.success() {
            return Err(Error::CommandFailed { check, status });
        }
        Ok(bytes)
    })
    .await;
    let result = result.unwrap_or(Err(Error::Timeout { check }));
    if result.is_err()
        && child.id().is_some()
        && let Err(error) = child.kill().await
    {
        warn!(%error, %check, "Failed to stop Claude ACP inspection process.");
    }
    result
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
