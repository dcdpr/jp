use confique::Config as Confique;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// LLM configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Editor {
    /// The command to use for editing text.
    ///
    /// If unset, falls back to `env_vars`.
    pub cmd: Option<String>,

    /// The environment variables to use for editing text. Used if `cmd` is
    /// unset.
    ///
    /// Defaults to `JP_EDITOR`, `VISUAL`, and `EDITOR`.
    #[config(default = ["JP_EDITOR", "VISUAL", "EDITOR"])]
    pub env_vars: Vec<String>,
}

impl AssignKeyValue for <Editor as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "cmd" => self.cmd = kv.try_into_string().map(|v| (!v.is_empty()).then_some(v))?,
            "env_vars" => {
                kv.try_set_or_merge_vec(self.env_vars.get_or_insert_default(), |v| match v {
                    Value::String(v) => Ok(v),
                    _ => Err("Expected string".into()),
                })?;
            }

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
