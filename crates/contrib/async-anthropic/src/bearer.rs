//! Bearer-token (OAuth) authentication mode.
//!
//! Anthropic's subscription plans authenticate with an OAuth bearer token
//! instead of an API key, and only accept requests that carry the Claude Code
//! client fingerprint alongside the token.
//! This module holds that fingerprint as named constants and assembles the
//! request headers for bearer mode.
//!
//! The constants mirror the reference implementation pinned by RFD 090:
//! <https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/providers/anthropic.ts>
//!
//! How much of the fingerprint Anthropic actually enforces is an open
//! measurement (RFD 090, Phase 1); trim or extend these constants once that
//! measurement lands.

use reqwest::header::{HeaderMap, HeaderValue};

/// Beta identifier that switches the API into OAuth mode.
///
/// Required on every bearer-token request.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Claude Code beta set sent alongside [`OAUTH_BETA`] on bearer requests.
pub const CLAUDE_CODE_BETAS: &[&str] = &[
    "claude-code-20250219",
    "interleaved-thinking-2025-05-14",
    "context-management-2025-06-27",
];

/// Claude Code CLI version impersonated by the fingerprint.
pub const CLAUDE_CODE_VERSION: &str = "2.1.165";

/// Claude Agent SDK version embedded in the user agent.
pub const CLAUDE_AGENT_SDK_VERSION: &str = "0.3.165";

/// Claude client version sent as `anthropic-client-version`.
pub const CLAUDE_CLIENT_VERSION: &str = "1.11187.4";

/// Client identity: `x-app` header value.
pub const X_APP: &str = "cli";

/// Client identity: `anthropic-client-platform` header value.
pub const CLIENT_PLATFORM: &str = "desktop_app";

/// Stainless SDK fingerprint headers with static values.
///
/// The Anthropic TypeScript SDK stamps every request with `X-Stainless-*`
/// telemetry; Claude Code inherits them, so they are part of the observable
/// fingerprint.
pub const STAINLESS_HEADERS: &[(&str, &str)] = &[
    ("x-stainless-retry-count", "0"),
    ("x-stainless-runtime-version", "v24.3.0"),
    ("x-stainless-package-version", "0.94.0"),
    ("x-stainless-runtime", "node"),
    ("x-stainless-lang", "js"),
    ("x-stainless-timeout", "900"),
];

/// Build the Claude Code user agent string.
#[must_use]
pub fn user_agent() -> String {
    format!(
        "claude-cli/{CLAUDE_CODE_VERSION} (external, local-agent, \
         agent-sdk/{CLAUDE_AGENT_SDK_VERSION})"
    )
}

/// Map the compile-time OS to the `X-Stainless-OS` header value.
#[must_use]
pub fn stainless_os() -> String {
    match std::env::consts::OS {
        "macos" => "MacOS".to_owned(),
        "windows" => "Windows".to_owned(),
        "linux" => "Linux".to_owned(),
        "freebsd" => "FreeBSD".to_owned(),
        other => format!("Other::{other}"),
    }
}

/// Map the compile-time architecture to the `X-Stainless-Arch` header value.
#[must_use]
pub fn stainless_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_owned(),
        "aarch64" => "arm64".to_owned(),
        "x86" => "x86".to_owned(),
        other => format!("other::{other}"),
    }
}

/// Merge the fingerprint betas with user-configured extras.
///
/// Fingerprint betas come first and cannot be removed; extras are appended in
/// order, with duplicates (of the fingerprint set or each other) dropped.
#[must_use]
pub fn merge_betas(extra: Option<&str>) -> String {
    let mut merged: Vec<&str> = Vec::new();
    let base = std::iter::once(OAUTH_BETA).chain(CLAUDE_CODE_BETAS.iter().copied());
    let extras = extra
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty());

    for beta in base.chain(extras) {
        if !merged.contains(&beta) {
            merged.push(beta);
        }
    }

    merged.join(",")
}

/// Assemble the bearer-mode request headers.
///
/// Emits `Authorization: Bearer <token>` (never `x-api-key`), the merged
/// `anthropic-beta` set, and the Claude Code client fingerprint, including a
/// fresh `x-client-request-id` per call.
pub(crate) fn headers(token: &str, version: &str, extra_betas: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let mut insert = |name: &'static str, value: String| {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(name, value);
        }
    };

    insert("authorization", format!("Bearer {token}"));
    insert("anthropic-version", version.to_owned());
    insert("anthropic-beta", merge_betas(extra_betas));
    insert("anthropic-dangerous-direct-browser-access", "true".into());
    insert("anthropic-client-platform", CLIENT_PLATFORM.to_owned());
    insert("anthropic-client-version", CLAUDE_CLIENT_VERSION.to_owned());
    insert("user-agent", user_agent());
    insert("x-app", X_APP.to_owned());
    insert("x-client-request-id", uuid::Uuid::new_v4().to_string());
    insert("x-stainless-os", stainless_os());
    insert("x-stainless-arch", stainless_arch());
    for (name, value) in STAINLESS_HEADERS {
        insert(name, (*value).to_owned());
    }

    headers
}
