//! Template configuration for Jean-Pierre.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_mergeable_value_map},
    fill::FillDefaults,
    internal::merge::map_with_strategy,
    partial::ToPartial,
    types::{json_value::JsonValue, map::MergeableMap},
};

/// Template configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TemplateConfig {
    /// Template variable values used to render query templates.
    ///
    /// Entries merge by key, so a value set in a later layer joins the ones an
    /// earlier layer set.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// them instead.
    #[setting(nested, merge = map_with_strategy)]
    pub values: MergeableMap<JsonValue>,
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
            values: delta_mergeable_value_map(&self.values, next.values),
        }
    }
}

impl FillDefaults for PartialTemplateConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            values: self.values.fill_from(defaults.values),
        }
    }
}

impl ToPartial for TemplateConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            // Per key rather than `replace`: a value the workspace config
            // gained after this conversation was created still reaches it.
            values: MergeableMap::Map(
                self.values
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
