//! Anthropic credential mechanics.
//!
//! Account identity recovery via the Claude CLI bootstrap endpoint: given a
//! fresh access or setup token, the endpoint reports the account UUID and email
//! the token belongs to.
//! Login uses it when the token response carries no identity; a profile whose
//! recovery fails is stored unverified.

use std::time::Duration;

use async_anthropic::bearer;
use async_trait::async_trait;
use serde::Deserialize;

use crate::credential::{AccountIdentity, ProviderAuth};

/// The Claude CLI bootstrap endpoint.
///
/// Whether it accepts long-lived setup tokens (not just OAuth access tokens) is
/// an open measurement (RFD 090, Phase 1); a rejection lands on the
/// unverified-profile path either way.
const BOOTSTRAP_URL: &str = "https://api.anthropic.com/api/claude_cli/bootstrap";

/// Query parameters the reference implementation pins on the bootstrap call.
const BOOTSTRAP_QUERY: &[(&str, &str)] = &[("entrypoint", "cli"), ("model", "claude-opus-4-8")];

const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

/// Anthropic's [`ProviderAuth`] implementation.
#[derive(Debug, Clone, Copy)]
pub struct AnthropicAuth;

#[async_trait]
impl ProviderAuth for AnthropicAuth {
    fn setup_token_hint(&self) -> &'static str {
        "Run `claude setup-token` on its own, complete the sign-in, then paste the \
         `sk-ant-oat01-…` value it prints. That command is an interactive session, so it cannot be \
         the source of a pipe: every stage of a pipeline starts at once, so JP would read the \
         stream before a token exists. A non-interactive source pipes fine, e.g. `pbpaste | jp \
         provider llm auth login anthropic --setup-token`."
    }

    async fn recover_identity(
        &self,
        token: &str,
    ) -> Result<AccountIdentity, Box<dyn std::error::Error + Send + Sync>> {
        let client = bootstrap_client()?;

        let response = client
            .get(BOOTSTRAP_URL)
            .query(BOOTSTRAP_QUERY)
            .header("accept", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("anthropic-beta", bearer::OAUTH_BETA)
            .header(
                "user-agent",
                format!("claude-code/{}", bearer::CLAUDE_CODE_VERSION),
            )
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(format!("bootstrap endpoint answered HTTP {status}: {body}").into());
        }

        Ok(parse_bootstrap_response(&body))
    }
}

/// Build the HTTP client used for the bootstrap call.
fn bootstrap_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(BOOTSTRAP_TIMEOUT)
        .build()
}

#[derive(Deserialize)]
struct BootstrapResponse {
    #[serde(default)]
    oauth_account: Option<BootstrapAccount>,
}

#[derive(Deserialize)]
struct BootstrapAccount {
    #[serde(default)]
    account_uuid: Option<String>,
    #[serde(default)]
    account_email: Option<String>,
}

/// Extract the account identity from a bootstrap response body.
///
/// Missing or empty fields become `None`; an unparseable body is an empty
/// identity rather than an error, since the response carries best-effort
/// metadata only.
fn parse_bootstrap_response(body: &str) -> AccountIdentity {
    let account = serde_json::from_str::<BootstrapResponse>(body)
        .ok()
        .and_then(|response| response.oauth_account);

    let non_empty = |v: Option<String>| v.filter(|s| !s.is_empty());

    account.map_or_else(AccountIdentity::default, |account| AccountIdentity {
        account_id: non_empty(account.account_uuid),
        email: non_empty(account.account_email),
    })
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
