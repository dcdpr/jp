//! Template configuration for Jean-Pierre.

use indexmap::IndexMap;
use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_value_map, delta_value_map_with_unsets, path},
    fill::FillDefaults,
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
            values: delta_value_map(&self.values, next.values),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            values: delta_value_map_with_unsets(
                &path(prefix, "values"),
                &self.values,
                next.values,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialTemplateConfig {
    fn fill_from(self, _defaults: Self) -> Self {
        self
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
