//! Template configuration for Jean-Pierre.

use schematic::Config;
use serde_json::{Map, Value};

use crate::{
    assignment::{missing_key, type_error, AssignKeyValue, KvAssignment, KvValue},
    delta::{delta_opt, PartialConfigDelta},
    partial::{partial_opt, ToPartial},
    BoxedError,
};

/// Template configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct TemplateConfig {
    /// Template variable values used to render query templates.
    // #[setting(nested)] TODO
    pub values: Map<String, Value>,
}

impl AssignKeyValue for PartialTemplateConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), BoxedError> {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("values") => {
                let remaining_key = kv.key_string();
                if remaining_key.is_empty() {
                    return type_error(kv.key(), &kv.value, &["object"]).map_err(Into::into);
                }

                let values = self.values.get_or_insert_default();
                let value = match kv.value {
                    KvValue::Json(v) => v,
                    KvValue::String(s) => Value::String(s),
                };

                let mut current = values;
                let mut parts = remaining_key.split('.').peekable();
                while let Some(part) = parts.next() {
                    if parts.peek().is_none() {
                        current.insert(part.to_string(), value);
                        break;
                    }

                    let entry = current
                        .entry(part.to_string())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));

                    if let Value::Object(obj) = entry {
                        current = obj;
                    } else {
                        return Err("Cannot set nested value on non-object".into());
                    }
                }
            }
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTemplateConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            values: delta_opt(self.values.as_ref(), next.values),
        }
    }
}

impl ToPartial for TemplateConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            values: partial_opt(&self.values, defaults.values),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assignment::KvAssignment;

    #[test]
    fn test_template_config_values() {
        let mut p = PartialTemplateConfig::default();

        let kv = KvAssignment::try_from_cli("values.name", "Homer").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.values.as_ref().unwrap().get("name"),
            Some(&Value::String("Homer".to_string()))
        );
    }
}
