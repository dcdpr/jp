//! Template configuration for Jean-Pierre.

use indexmap::IndexMap;
use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
    types::json_value::JsonValue,
    util::merge_nested_indexmap,
};

/// Template configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TemplateConfig {
    /// Template variable values used to render query templates.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub values: IndexMap<String, JsonValue>,
}

impl AssignKeyValue for PartialTemplateConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), crate::BoxedError> {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("values") => kv.assign_to_entry(&mut self.values)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTemplateConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            values: next
                .values
                .into_iter()
                .filter_map(|(name, next)| {
                    if self.values.get(&name).is_some_and(|prev| prev == &next) {
                        return None;
                    }
                    Some((name, next))
                })
                .collect(),
        }
    }
}

impl ToPartial for TemplateConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            values: self
                .values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
