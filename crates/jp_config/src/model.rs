pub mod parameters;

use confique::Config as Confique;
use jp_model::ModelId;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    serde::{de_from_str_opt, is_nested_default_or_empty},
    Error,
};

/// Model configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Model {
    /// Model to use.
    #[config(partial_attr(serde(
        default,
        deserialize_with = "de_from_str_opt",
        skip_serializing_if = "Option::is_none"
    )))]
    pub id: Option<ModelId>,

    /// The parameters to use for the model.
    #[config(
        nested,
        partial_attr(serde(skip_serializing_if = "is_nested_default_or_empty"))
    )]
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

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
