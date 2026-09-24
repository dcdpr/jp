//! HTTP client construction for API-key and direct subscription requests.

use std::sync::{Arc, Mutex};

use async_anthropic::{Client, bearer, errors::AnthropicError};
use jp_config::providers::llm::anthropic::AnthropicConfig;
use tracing::debug;

use crate::{
    credential::Credential,
    error::{Error, Result},
};

/// Retains a connection pool while the selected credential is unchanged.
#[derive(Debug, Clone, Default)]
pub(super) struct Clients {
    cached: Arc<Mutex<Option<(Credential, Client, bool)>>>,
}

impl Clients {
    /// Build the client and report whether requests require bearer shaping.
    pub(super) fn get(
        &self,
        config: &AnthropicConfig,
        credential: &Credential,
    ) -> Result<(Client, bool)> {
        let mut cache = self.cached.lock().expect("poisoned");
        if let Some((cached, client, bearer)) = cache.as_ref()
            && cached == credential
        {
            return Ok((client.clone(), *bearer));
        }
        let mut builder = Client::builder();
        builder
            .base_url(config.base_url.clone())
            .version("2023-06-01");
        let bearer = match credential {
            Credential::ApiKey(key) => {
                builder.api_key(key.clone());
                false
            }
            Credential::Bearer(token) => {
                builder.auth_token(token.clone());
                true
            }
        };
        if !config.beta_headers.is_empty() {
            builder.beta(config.beta_headers.join(","));
        }
        debug!(
            bearer,
            betas = %if bearer {
                bearer::merge_betas((!config.beta_headers.is_empty()).then(|| config.beta_headers.join(",")).as_deref())
            } else {
                config.beta_headers.join(",")
            },
            "Constructing Anthropic client."
        );
        let client = builder
            .build()
            .map_err(|e| Error::Anthropic(AnthropicError::Unknown(e.to_string())))?;
        *cache = Some((credential.clone(), client.clone(), bearer));
        Ok((client, bearer))
    }
}
