//! `OpenAI`'s OAuth flow: PKCE, device authorization, token exchange, and
//! refresh.
//!
//! The endpoints and client id are the Codex CLI's, not published API.
//! They are named here as constants so a drift shows up in one place.
//!
//! Request building, response parsing, and JWT claim extraction are pure;
//! [`exchange_code`], [`refresh`], [`start_device_auth`], and
//! [`poll_device_auth`] are the thin network shell around them.
//! The browser, the localhost callback, and the store writes live outside this
//! module.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::credential::AccountIdentity;

/// The Codex CLI's OAuth client id.
///
/// JP borrows it: `OpenAI` issues subscription inference credentials only to
/// this client, and there is no way to register another.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The OAuth issuer.
const ISSUER: &str = "https://auth.openai.com";

/// The scopes JP asks for.
///
/// `openid profile email` is what puts the account id in the `id_token`;
/// `offline_access` is what yields a refresh token.
const SCOPES: &str = "openid profile email offline_access";

/// The port the localhost callback listens on.
///
/// Fixed rather than ephemeral: the redirect URI is registered against the
/// client id, so another port is refused.
pub const CALLBACK_PORT: u16 = 1455;

/// The path the localhost callback is served at.
pub const CALLBACK_PATH: &str = "/auth/callback";

/// How JP identifies itself to the subscription endpoint.
///
/// Deliberately JP's own name rather than the Codex CLI's: `OpenAI`'s client
/// code distinguishes first-party originators, and claiming to be one would be
/// a misrepresentation.
/// A third-party value is accepted.
pub const ORIGINATOR: &str = "jp";

const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long before expiry an access token is treated as stale.
///
/// Refreshing early keeps a token from expiring between resolution and the
/// request it was resolved for.
pub const EXPIRY_BUFFER: TimeDelta = TimeDelta::minutes(5);

/// Fallback access-token lifetime when the response omits `expires_in`.
const DEFAULT_EXPIRES_IN: i64 = 3600;

/// Errors from the OAuth endpoints.
#[derive(Debug, thiserror::Error)]
pub enum OauthError {
    #[error("could not reach the OpenAI OAuth endpoint")]
    Transport(#[from] reqwest::Error),

    /// The endpoint refused the grant.
    ///
    /// A refused refresh token cannot be recovered by retrying; the profile
    /// needs a fresh login.
    #[error("OAuth request rejected (HTTP {status}): {body}")]
    Rejected { status: u16, body: String },

    #[error("could not parse the OAuth token response: {0}")]
    Malformed(#[from] serde_json::Error),

    /// The device flow was still pending when the caller stopped waiting.
    #[error("device authorization timed out; run the login again")]
    DeviceTimeout,
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

    /// Reconstruct a PKCE pair from a verifier the server generated.
    ///
    /// The device flow returns the verifier alongside the authorization code,
    /// so the challenge is never needed on that path.
    #[must_use]
    pub fn from_verifier(verifier: String) -> Self {
        Self {
            verifier,
            challenge: String::new(),
        }
    }
}

/// The tokens an exchange or refresh produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,

    /// The account the tokens belong to, read from the JWT claims.
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
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The localhost redirect the browser flow captures the code on.
#[must_use]
pub fn callback_redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}")
}

/// The URL the user opens to approve the grant.
#[must_use]
pub fn authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPES),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        // Puts the account's organizations in the id_token, which is the
        // fallback when no `chatgpt_account_id` claim is present.
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", ORIGINATOR),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", urlencode(value)))
    .collect::<Vec<_>>()
    .join("&");

    format!("{ISSUER}/oauth/authorize?{query}")
}

/// The page the user opens to enter a device code.
#[must_use]
pub fn device_verification_url() -> String {
    format!("{ISSUER}/codex/device")
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

/// The form body of an authorization-code exchange.
///
/// The token endpoint takes form encoding, not JSON.
#[must_use]
pub fn exchange_form(code: &str, verifier: &str, redirect_uri: &str) -> Vec<(String, String)> {
    vec![
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("code".to_owned(), code.to_owned()),
        ("redirect_uri".to_owned(), redirect_uri.to_owned()),
        ("client_id".to_owned(), CLIENT_ID.to_owned()),
        ("code_verifier".to_owned(), verifier.to_owned()),
    ]
}

/// The form body of a refresh-token exchange.
#[must_use]
pub fn refresh_form(refresh_token: &str) -> Vec<(String, String)> {
    vec![
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("refresh_token".to_owned(), refresh_token.to_owned()),
        ("client_id".to_owned(), CLIENT_ID.to_owned()),
    ]
}

/// Exchange an authorization code for tokens.
///
/// # Errors
///
/// Returns an error when the endpoint cannot be reached, refuses the code, or
/// answers with a body this build cannot read.
pub async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, OauthError> {
    post_token(exchange_form(code, verifier, redirect_uri), Utc::now()).await
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
    let mut tokens = post_token(refresh_form(refresh_token), Utc::now()).await?;

    // A response that omits `refresh_token` leaves the current one in force.
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = refresh_token.to_owned();
    }

    Ok(tokens)
}

/// A started device authorization: the code to show the user, and what polling
/// needs.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuth {
    /// The server's handle for this authorization attempt.
    pub device_auth_id: String,

    /// The code the user types into the verification page.
    pub user_code: String,

    /// How long to wait between polls, in seconds, as a string.
    #[serde(default)]
    interval: String,
}

impl DeviceAuth {
    /// How long to wait between polls.
    ///
    /// A safety margin is added to the server's interval, since polling faster
    /// than asked is answered with a refusal.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        let secs = self.interval.trim().parse::<u64>().unwrap_or(5).max(1);

        Duration::from_secs(secs) + Duration::from_secs(3)
    }
}

/// Begin a device authorization.
///
/// # Errors
///
/// Returns an error when the endpoint cannot be reached or refuses to start a
/// device authorization, which is what an account without device login enabled
/// answers.
pub async fn start_device_auth() -> Result<DeviceAuth, OauthError> {
    let body = serde_json::json!({ "client_id": CLIENT_ID });

    let response = client()?
        .post(format!("{ISSUER}/api/accounts/deviceauth/usercode"))
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

    Ok(serde_json::from_str(&text)?)
}

/// The outcome of one device-authorization poll.
#[derive(Debug)]
pub enum DevicePoll {
    /// The user has not finished approving yet.
    Pending,

    /// The user approved; these are the tokens.
    Ready(Tokens),
}

/// Ask once whether the device authorization has been approved.
///
/// A `403` or `404` means "not yet" and is reported as [`DevicePoll::Pending`];
/// every other refusal is an error.
///
/// # Errors
///
/// Returns an error when the endpoint cannot be reached, refuses the
/// authorization outright, or answers with a body this build cannot read.
pub async fn poll_device_auth(device: &DeviceAuth) -> Result<DevicePoll, OauthError> {
    let body = serde_json::json!({
        "device_auth_id": device.device_auth_id,
        "user_code": device.user_code,
    });

    let response = client()?
        .post(format!("{ISSUER}/api/accounts/deviceauth/token"))
        .json(&body)
        .send()
        .await?;

    let status = response.status();

    if status.as_u16() == 403 || status.as_u16() == 404 {
        return Ok(DevicePoll::Pending);
    }

    let text = response.text().await?;

    if !status.is_success() {
        return Err(OauthError::Rejected {
            status: status.as_u16(),
            body: text,
        });
    }

    let granted: DeviceGrant = serde_json::from_str(&text)?;
    let redirect = format!("{ISSUER}/deviceauth/callback");
    let tokens = exchange_code(
        &granted.authorization_code,
        &granted.code_verifier,
        &redirect,
    )
    .await?;

    Ok(DevicePoll::Ready(tokens))
}

/// What an approved device authorization hands back.
#[derive(Debug, Deserialize)]
struct DeviceGrant {
    authorization_code: String,
    code_verifier: String,
}

/// Build the HTTP client used for the OAuth endpoints.
fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder().timeout(TOKEN_TIMEOUT).build()
}

/// Send a token request and read the response.
async fn post_token(form: Vec<(String, String)>, now: DateTime<Utc>) -> Result<Tokens, OauthError> {
    let response = client()?
        .post(format!("{ISSUER}/oauth/token"))
        .form(&form)
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

    #[serde(default)]
    id_token: String,

    /// Lifetime of the access token, in seconds.
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Parse a token response, resolving its relative expiry against `now`.
pub(super) fn parse_tokens(body: &str, now: DateTime<Utc>) -> Result<Tokens, OauthError> {
    let response: TokenResponse = serde_json::from_str(body)?;

    let identity = identity_from_tokens(&response.id_token, &response.access_token);
    let expires_in = response.expires_in.unwrap_or(DEFAULT_EXPIRES_IN);

    Ok(Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: now + TimeDelta::seconds(expires_in),
        identity,
    })
}

/// Read the account identity out of a token pair.
///
/// The `id_token` is authoritative; the access token carries the same claims
/// and is the fallback when no id token was issued (a refresh often omits it).
#[must_use]
pub fn identity_from_tokens(id_token: &str, access_token: &str) -> AccountIdentity {
    let identity = claims_identity(id_token);

    if identity.account_id.is_some() {
        return identity;
    }

    let fallback = claims_identity(access_token);

    AccountIdentity {
        account_id: fallback.account_id,
        email: identity.email.or(fallback.email),
    }
}

/// Extract the account identity from one JWT's payload.
///
/// A token that is not a JWT, or whose payload this build cannot read, yields
/// an empty identity rather than an error: the claims are best-effort metadata,
/// and a credential without them still authenticates requests.
fn claims_identity(token: &str) -> AccountIdentity {
    let Some(claims) = jwt_claims(token) else {
        return AccountIdentity::default();
    };

    let string = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    // `chatgpt_account_id` sits at the top level on some tokens and under the
    // namespaced auth claim on others; the first organization is the last
    // resort, which `id_token_add_organizations=true` is what supplies.
    let nested = claims.get("https://api.openai.com/auth");
    let account_id = string(claims.get("chatgpt_account_id"))
        .or_else(|| string(nested.and_then(|auth| auth.get("chatgpt_account_id"))))
        .or_else(|| {
            string(
                claims
                    .get("organizations")
                    .and_then(Value::as_array)
                    .and_then(|orgs| orgs.first())
                    .and_then(|org| org.get("id")),
            )
        });

    AccountIdentity {
        account_id,
        email: string(claims.get("email")),
    }
}

/// Decode a JWT's payload without verifying its signature.
///
/// JP does not validate the token: it was just received over TLS from the
/// endpoint that minted it, and the claims are read for display and
/// duplicate-account detection rather than for authorization.
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;

    serde_json::from_slice(&bytes).ok()
}

/// The compute-residency constraint a token carries, if any.
///
/// Requests for a residency-constrained account are refused without the
/// matching header.
#[must_use]
pub fn residency(access_token: &str) -> Option<String> {
    let claims = jwt_claims(access_token)?;

    let read = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "no_constraint")
            .map(str::to_owned)
    };

    read(
        claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_compute_residency")),
    )
    .or_else(|| read(claims.get("chatgpt_compute_residency")))
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
