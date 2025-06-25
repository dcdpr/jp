pub mod parameters;

use std::str::FromStr;

use confique::Config as Confique;
use jp_model::ModelId;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    is_empty, Error,
};

/// Model configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Model {
    /// Model to use.
    #[config(partial_attr(serde(default, deserialize_with = "de_option_model_id")))]
    pub id: Option<ModelId>,

    /// The parameters to use for the model.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub parameters: parameters::Parameters,
}

impl AssignKeyValue for <Model as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), Error> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "parameters" => self.parameters = kv.try_into_object()?,
            "id" => {
                self.id = kv
                    .try_into_string()
                    .map(|v| (!v.is_empty()).then(|| v.parse()))?
                    .transpose()?;
            }

            _ if kv.trim_prefix("parameters") => self.parameters.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}

pub fn de_option_model_id<'de, D>(deserializer: D) -> Result<Option<ModelId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|v| ModelId::from_str(&v))
        .transpose()
        .map_err(serde::de::Error::custom)
}
