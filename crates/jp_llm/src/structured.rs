//! Tools for requesting structured data from LLMs using tool calls.

pub mod titles;

use jp_config::model::{id::ModelIdConfig, parameters::ParametersConfig};
use serde::de::DeserializeOwned;

use crate::{error::Result, provider::Provider, query::StructuredQuery};

// Name of the schema enforcement tool
pub(crate) const SCHEMA_TOOL_NAME: &str = "generate_structured_data";

/// Request structured data from the LLM for any type `T` that implements
/// [`DeserializeOwned`].
///
/// It assumes a [`StructuredQuery`] that has a schema to enforce the correct
/// sturcute for `T`.
///
/// If a LLM model enforces a JSON object as the response, but you want (e.g.) a
/// list of items, you can use [`StructuredQuery::with_mapping`] to map the
/// response object into the final shape of `T`.
pub async fn completion<T: DeserializeOwned>(
    provider: &dyn Provider,
    model_id: &ModelIdConfig,
    parameters: &ParametersConfig,
    query: StructuredQuery,
) -> Result<T> {
    let value = provider
        .structured_completion(model_id, parameters, query)
        .await?;

    serde_json::from_value(value).map_err(Into::into)
}
