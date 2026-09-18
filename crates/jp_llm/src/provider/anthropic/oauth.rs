//! Anthropic's OAuth flow: PKCE, token exchange, and refresh.
//!
//! The endpoints, client id, and request shapes are Claude Code's, not
//! published API (RFD 090).
//! They are named here as constants so a drift shows up in one place.
//!
//! Request building and response parsing are pure; [`exchange_code`] and
//! [`refresh`] are the thin network shell around them.
//! The browser, the localhost callback, and the store writes live outside this
//! module.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::credential::AccountIdentity;

/// Claude Code's OAuth client id.
///
/// JP borrows it: Anthropic issues `user:inference` only to this client, and
/// there is no way to register another (RFD 090, Policy risk).
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Where the user approves the grant.
///
/// Anthropic routes this through `claude.com/cai/*`, which redirects to
/// `claude.ai/oauth/authorize`.
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";

/// Where an authorization code and a refresh token are exchanged for tokens.
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// The redirect used when no localhost callback can be reached.
///
/// The page shows the code for the user to paste back.
pub const MANUAL_REDIRECT_URL: &str = "https://platform.claude.com/oauth/code/callback";

/// The scopes JP asks for.
///
/// Deliberately smaller than Claude Code's own grant, which also requests
/// API-key creation, session, MCP-server, and file-upload capabilities that JP
/// has no use for.
/// `user:profile` is not optional within this set: account attribution needs it
/// (RFD 090, Phase 1).
const SCOPES: &[&str] = &["user:profile", "user:inference"];

const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long before expiry an access token is treated as stale.
///
/// Refreshing early keeps a token from expiring between resolution and the
/// request it was resolved for.
pub const EXPIRY_BUFFER: TimeDelta = TimeDelta::minutes(5);

/// Errors from the OAuth endpoints.
#[derive(Debug, thiserror::Error)]
pub enum OauthError {
    #[error("could not reach the Anthropic OAuth endpoint")]
    Transport(#[from] reqwest::Error),

    /// The endpoint refused the grant.
    ///
    /// A refused refresh token cannot be recovered by retrying; the profile
    /// needs a fresh login.
    #[error("OAuth request rejected (HTTP {status}): {body}")]
    Rejected { status: u16, body: String },

    #[error("could not parse the OAuth token response: {0}")]
    Malformed(#[from] serde_json::Error),
}

impl OauthError {
    /// Whether the grant itself was refused, as opposed to the request not
    /// getting through.
    ///
    /// Only a refusal means the stored credential is dead; a transport failure
    /// is worth another attempt later.
    #[must_use]
    pub fn is_rejection(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

/// A PKCE verifier and the challenge derived from it.
#[derive(Debug, Clone)]
pub struct Pkce {
    /// The secret held by the client until the code is exchanged.
    pub verifier: String,

    /// The `S256` hash of the verifier, sent with the authorize request.
    pub challenge: String,
}

impl Pkce {
    /// Generate a fresh verifier and its challenge.
    #[must_use]
    pub fn generate() -> Self {
        let verifier = random_token();
        let digest = Sha256::digest(verifier.as_bytes());

        Self {
            challenge: base64_url(&digest),
            verifier,
        }
    }
}

/// The tokens an exchange or refresh produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,

    /// The account the tokens belong to, when the response named it.
    pub identity: AccountIdentity,
}

/// 256 bits of URL-safe randomness, for a PKCE verifier or an OAuth `state`.
///
/// Sourced from v4 UUIDs, which are drawn from the platform's cryptographic
/// random number generator.
#[must_use]
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());

    base64_url(&bytes)
}

/// Base64 encode without padding, using the URL-safe alphabet.
fn base64_url(bytes: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    URL_SAFE_NO_PAD.encode(bytes)
}

/// The URL the user opens to approve the grant.
///
/// `redirect_uri` is the localhost callback, or [`MANUAL_REDIRECT_URL`] when no
/// callback can be reached.
#[must_use]
pub fn authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let query = [
        // Tells the consent page this is a CLI code flow.
        ("code", "true"),
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("scope", &SCOPES.join(" ")),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", urlencode(value)))
    .collect::<Vec<_>>()
    .join("&");

    format!("{AUTHORIZE_URL}?{query}")
}

/// Percent-encode a query parameter value.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The body of an authorization-code exchange.
#[must_use]
pub fn exchange_body(code: &str, state: &str, verifier: &str, redirect_uri: &str) -> Value {
    json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": CLIENT_ID,
        "code_verifier": verifier,
        "state": state,
    })
}

/// The body of a refresh-token exchange.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> Value {
    json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
        "scope": SCOPES.join(" "),
    })
}

/// Exchange an authorization code for tokens.
///
/// # Errors
///
/// Returns an error when the endpoint cannot be reached, refuses the code, or
/// answers with a body this build cannot read.
pub async fn exchange_code(
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, OauthError> {
    post_token(
        exchange_body(code, state, verifier, redirect_uri),
        Utc::now(),
    )
    .await
}

/// Trade a refresh token for a fresh access token.
///
/// The refresh token rotates: the response usually carries a new one, and the
/// old one stops working.
/// Persisting the result is the caller's job, and must happen under the store
/// lock.
///
/// # Errors
///
/// Returns an error when the endpoint cannot be reached, refuses the token, or
/// answers with a body this build cannot read.
pub async fn refresh(refresh_token: &str) -> Result<Tokens, OauthError> {
    let mut tokens = post_token(refresh_body(refresh_token), Utc::now()).await?;

    // A response that omits `refresh_token` leaves the current one in force.
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = refresh_token.to_owned();
    }

    Ok(tokens)
}

/// Send a token request and read the response.
async fn post_token(body: Value, now: DateTime<Utc>) -> Result<Tokens, OauthError> {
    let response = reqwest::Client::builder()
        .timeout(TOKEN_TIMEOUT)
        .build()?
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    let text = response.text().await?;

    if !status.is_success() {
        return Err(OauthError::Rejected {
            status: status.as_u16(),
            body: text,
        });
    }

    parse_tokens(&text, now)
}

/// The token endpoint's response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,

    #[serde(default)]
    refresh_token: String,

    /// Lifetime of the access token, in seconds.
    expires_in: i64,

    #[serde(default)]
    account: Option<TokenAccount>,
}

#[derive(Debug, Deserialize)]
struct TokenAccount {
    #[serde(default)]
    uuid: Option<String>,

    #[serde(default)]
    email_address: Option<String>,
}

/// Parse a token response, resolving its relative expiry against `now`.
fn parse_tokens(body: &str, now: DateTime<Utc>) -> Result<Tokens, OauthError> {
    let response: TokenResponse = serde_json::from_str(body)?;

    let non_empty = |value: Option<String>| value.filter(|v| !v.is_empty());
    let identity = response
        .account
        .map_or_else(AccountIdentity::default, |account| AccountIdentity {
            account_id: non_empty(account.uuid),
            email: non_empty(account.email_address),
        });

    Ok(Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: now + TimeDelta::seconds(response.expires_in),
        identity,
    })
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
