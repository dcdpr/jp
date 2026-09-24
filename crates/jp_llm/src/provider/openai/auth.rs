//! `OpenAI` credential mechanics.
//!
//! Account identity comes out of the token itself: the `id_token` a login
//! returns carries the `ChatGPT` account id as a JWT claim, so identity
//! recovery is a local decode rather than a network call.
//!
//! Also here: reading the Codex CLI's own credential cache, which is how a
//! machine that already ran `codex login` can hand JP a working credential
//! without a second browser round-trip.

use async_trait::async_trait;
use camino::Utf8PathBuf;
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::{
    credential::{AccountIdentity, ProviderAuth},
    provider::openai::oauth,
};

/// `OpenAI`'s [`ProviderAuth`] implementation.
#[derive(Debug, Clone, Copy)]
pub struct OpenaiAuth;

#[async_trait]
impl ProviderAuth for OpenaiAuth {
    fn setup_token_hint(&self) -> &'static str {
        "OpenAI issues no long-lived setup token. Either run `jp provider llm auth login openai` \
         to sign in through the browser, or paste an access token from an existing Codex CLI \
         session with `jq -r .tokens.access_token ~/.codex/auth.json | jp provider llm auth login \
         openai --setup-token`. A pasted access token expires within the hour and cannot be \
         refreshed; `--import-codex` stores the refreshable token pair instead."
    }

    async fn recover_identity(
        &self,
        token: &str,
    ) -> Result<AccountIdentity, Box<dyn std::error::Error + Send + Sync>> {
        let identity = oauth::identity_from_tokens("", token);

        if identity.account_id.is_none() {
            return Err("the token carries no ChatGPT account claim".into());
        }

        Ok(identity)
    }
}

/// A credential read out of the Codex CLI's cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub identity: AccountIdentity,
}

/// Errors from importing the Codex CLI's cached credential.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not read {path}: {source}")]
    Read {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse {path}: {source}")]
    Parse {
        path: Utf8PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The file exists but holds an API key rather than a subscription login.
    #[error(
        "{path} holds no ChatGPT login; run `codex login` (not `codex login --with-api-key`) \
         first, or use `jp provider llm auth login openai` to sign in through JP"
    )]
    NoTokens { path: Utf8PathBuf },
}

/// Where the Codex CLI caches its credentials.
///
/// Honours `CODEX_HOME`, which is what the Codex CLI itself reads.
#[must_use]
pub fn codex_auth_path() -> Option<Utf8PathBuf> {
    let home = std::env::var("CODEX_HOME")
        .ok()
        .map(Utf8PathBuf::from)
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            Some(Utf8PathBuf::from(home).join(".codex"))
        })?;

    Some(home.join("auth.json"))
}

/// Read the Codex CLI's cached credential.
///
/// The token pair is copied, not moved: the Codex CLI keeps working until JP
/// refreshes, at which point the rotated refresh token invalidates the session
/// on the other side.
///
/// # Errors
///
/// Returns an error when the file cannot be read or parsed, or when it holds no
/// `ChatGPT` login.
pub fn import_codex_credential(path: &Utf8PathBuf) -> Result<ImportedCredential, ImportError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ImportError::Read {
        path: path.clone(),
        source,
    })?;

    parse_codex_auth(&raw, path, Utc::now())
}

/// The shape of the Codex CLI's `auth.json`.
#[derive(Debug, Deserialize)]
struct CodexAuth {
    #[serde(default)]
    tokens: Option<CodexTokens>,

    /// When the CLI last refreshed, used to place the access token's expiry.
    #[serde(default)]
    last_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    #[serde(default)]
    access_token: String,

    #[serde(default)]
    refresh_token: String,

    #[serde(default)]
    id_token: String,

    #[serde(default)]
    account_id: Option<String>,
}

/// Parse a Codex `auth.json` body.
///
/// The file records no access-token expiry, only when the CLI last refreshed.
/// The imported credential is therefore treated as due for refresh immediately:
/// the refresh token is what has lasting value, and one extra refresh at import
/// costs a single round-trip.
fn parse_codex_auth(
    raw: &str,
    path: &Utf8PathBuf,
    now: DateTime<Utc>,
) -> Result<ImportedCredential, ImportError> {
    let parsed: CodexAuth = serde_json::from_str(raw).map_err(|source| ImportError::Parse {
        path: path.clone(),
        source,
    })?;

    let tokens = parsed
        .tokens
        .filter(|tokens| !tokens.refresh_token.is_empty())
        .ok_or_else(|| ImportError::NoTokens { path: path.clone() })?;

    let mut identity = oauth::identity_from_tokens(&tokens.id_token, &tokens.access_token);
    identity.account_id = identity.account_id.or(tokens.account_id);

    // Placed in the past so the first request refreshes, which is also what
    // proves the imported refresh token works.
    let expires_at = parsed.last_refresh.unwrap_or(now) - TimeDelta::seconds(1);

    Ok(ImportedCredential {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        identity,
    })
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
