//! Resolved provider credentials and provider credential mechanics.
//!
//! A [`Credential`] is the outcome of a provider's own credential resolution:
//! each provider with a credential chain walks it per request and authenticates
//! with the credential it lands on.
//! No other component consumes this type.
//!
//! The provider-specific mechanics behind stored credentials — identity
//! recovery today, token exchange and refresh in later phases — live behind
//! [`ProviderAuth`], dispatched by [`provider_auth`] the same way
//! [`get_provider`] dispatches chat providers.
//! The auth CLI (`jp provider llm auth`) reaches them through this seam; when a
//! provider moves out of JP core and into a plugin, its `ProviderAuth`
//! implementation migrates with it.
//!
//! [`get_provider`]: crate::provider::get_provider

use std::fmt;

use async_trait::async_trait;
use jp_config::model::id::ProviderId;

use crate::provider::{anthropic, openai};

/// A resolved credential for an LLM provider.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// A per-token API key, sent as `x-api-key`.
    ApiKey(String),

    /// A subscription OAuth bearer token, sent as `Authorization: Bearer`
    /// together with whatever request fingerprint the provider requires.
    Bearer(String),
}

impl Credential {
    /// A short label for the credential's mechanism.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ApiKey(_) => "api_key",
            Self::Bearer(_) => "bearer",
        }
    }
}

/// Redacts the secret; only the mechanism is shown.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential::{}(REDACTED)", match self {
            Self::ApiKey(_) => "ApiKey",
            Self::Bearer(_) => "Bearer",
        })
    }
}

/// Account identity attached to a stored credential.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    /// The provider-side account UUID, the key for duplicate-account detection.
    pub account_id: Option<String>,

    /// The account email, for display only.
    pub email: Option<String>,
}

/// Provider-specific mechanics for stored credentials.
///
/// These flows run before any credential exists (logging in is what produces
/// one), so they cannot live on [`Provider`], whose construction requires an
/// already-resolved credential.
///
/// [`Provider`]: crate::Provider
#[async_trait]
pub trait ProviderAuth: Send + Sync {
    /// How a user obtains a long-lived setup token for this provider.
    ///
    /// Surfaced verbatim when JP prompts for a token and when the input it
    /// receives cannot be one, so it names the command to run, the shape of the
    /// value to paste, and any constraint on scripting the step.
    /// Provider-specific token vocabulary lives here rather than in the CLI,
    /// which knows only that some providers accept setup tokens.
    fn setup_token_hint(&self) -> &'static str;

    /// Recover the account identity behind an access or setup token.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider cannot be reached or rejects the
    /// token.
    /// Callers treat a failure as "identity unknown", not as a fatal condition:
    /// a credential without a recovered identity is stored unverified.
    async fn recover_identity(
        &self,
        token: &str,
    ) -> Result<AccountIdentity, Box<dyn std::error::Error + Send + Sync>>;
}

/// Get the credential mechanics for a provider.
///
/// Returns `None` for providers without stored-credential support; their
/// constructors read their own environment variables instead.
#[must_use]
pub fn provider_auth(id: ProviderId) -> Option<Box<dyn ProviderAuth>> {
    match id {
        ProviderId::Anthropic => Some(Box::new(anthropic::auth::AnthropicAuth)),
        ProviderId::Openai => Some(Box::new(openai::auth::OpenaiAuth)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
