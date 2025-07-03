use std::collections::HashMap;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment, KvValue},
    error::Result,
};

/// Template configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Template {
    /// Template variable values used to render query templates.
    #[config(default = {})]
    pub values: HashMap<String, Value>,
}

impl AssignKeyValue for <Template as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        // let KvAssignment { key, value, .. } = kv;

        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            _ if kv.trim_prefix("values") => {
                let mut parts = kv.key().segments().peekable();
                let mut template_values = serde_json::Map::new();
                let mut values = &mut template_values;

                while let Some(segment) = parts.next() {
                    if parts.peek().is_none() {
                        values.insert(segment.to_owned(), match kv.value().clone() {
                            KvValue::Json(v) => v,
                            KvValue::String(v) => serde_json::Value::String(v),
                        });
                        break;
                    }

                    let next_val = values
                        .entry(segment.to_owned())
                        .and_modify(|v| match v {
                            Value::Object(_) => {}
                            v => *v = serde_json::json!({}),
                        })
                        .or_insert(serde_json::json!({}));

                    values = match next_val {
                        Value::Object(map) => map,
                        _ => unreachable!(),
                    };
                }

                self.values.get_or_insert_default().extend(template_values);
            }
            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
