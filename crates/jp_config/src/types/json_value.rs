//! A recursive JSON value that participates in schematic's `Config` system.
//!
//! Wraps [`serde_json::Value`] with [`Config`], [`PartialConfig`], and
//! [`AssignKeyValue`] implementations. Merge behavior is delegated to the
//! existing `Mergeable*` types based on the JSON value type:
//!
//! - **Arrays** → [`MergeableVec`]
//! - **Objects** → [`MergeableMap`]
//! - **Strings** → [`MergeableString`]
//! - **Primitives** → last-writer-wins replace
//!
//! Each type supports an annotation shape `{ value = <V>, strategy = "<S>" }`
//! with type-appropriate strategies. See the respective module docs for
//! available strategies. Without an annotation, the defaults are:
//!
//! - Arrays: replace
//! - Objects: deep-merge
//! - Strings: replace
//!
//! [`MergeableString`]: super::string::MergeableString

use std::ops::{Deref, DerefMut};

use schematic::{Config, ConfigError, PartialConfig, Schema, SchemaBuilder, SchemaType, Schematic};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, from_value, to_value};

use super::{map::MergeableMap, string::PartialMergeableString, vec::MergeableVec};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    internal::merge::{map_with_strategy, string_with_strategy, vec_with_strategy},
    types::vec::{MergedVec, MergedVecStrategy},
};

/// A JSON value usable as a schematic [`Config`] type.
///
/// Transparently serializes/deserializes as a [`serde_json::Value`].
///
/// See the [module documentation](self) for merge semantics.
///
/// # Assignment
///
/// Implements [`AssignKeyValue`] for arbitrary dot-path assignment. Given
/// `options.web.port=3000`, the type builds the nested structure
/// `{"web": {"port": 3000}}` automatically. Intermediate objects are created
/// on demand.
///
/// # Partial identity
///
/// `JsonValue` is its own `Partial` type. The "not set" state is represented
/// by absence from the containing `IndexMap`, not by a sentinel value within
/// `JsonValue` itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonValue(pub Value);

impl Default for JsonValue {
    fn default() -> Self {
        Self(Value::Null)
    }
}

impl Deref for JsonValue {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

impl DerefMut for JsonValue {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}

impl From<Value> for JsonValue {
    fn from(v: Value) -> Self {
        Self(v)
    }
}

impl From<JsonValue> for Value {
    fn from(v: JsonValue) -> Self {
        v.0
    }
}

/// Merge `next` into `base` by delegating to the appropriate `Mergeable*` type.
///
/// Annotations (`{ value, strategy }`) are detected first so the dispatch
/// is based on the inner value type, not the wrapper object.
fn merge_values(base: &mut Value, next: Value) {
    // Peek: if next is an annotation, route by the inner value's type.
    let dispatch_type = if let Value::Object(ref map) = next
        && map.contains_key("strategy")
    {
        map.get("value").map(annotation_dispatch_type)
    } else {
        None
    };

    match dispatch_type.unwrap_or_else(|| annotation_dispatch_type(&next)) {
        DispatchType::Map => merge_as_map(base, next),
        DispatchType::Vec => merge_as_vec(base, next),
        DispatchType::String => merge_as_string(base, next),
        DispatchType::Primitive => *base = next,
    }
}

/// Which `Mergeable*` handler to use.
#[derive(Debug, Clone, Copy)]
enum DispatchType {
    /// [`MergeableMap`]
    Map,

    /// [`MergeableVec`]
    Vec,

    /// [`PartialMergeableString`]
    String,

    /// Last-writer-wins replace.
    Primitive,
}

/// Determine the dispatch type for a JSON value.
const fn annotation_dispatch_type(v: &Value) -> DispatchType {
    match v {
        Value::Object(_) => DispatchType::Map,
        Value::Array(_) => DispatchType::Vec,
        Value::String(_) => DispatchType::String,
        _ => DispatchType::Primitive,
    }
}

/// Merge as `MergeableMap<JsonValue>`.
fn merge_as_map(base: &mut Value, next: Value) {
    let Ok(next_map) = from_value::<MergeableMap<JsonValue>>(next.clone()) else {
        *base = next;
        return;
    };

    // Convert base to MergeableMap. Non-objects start empty.
    let base_val = std::mem::replace(base, Value::Null);
    let base_map: MergeableMap<JsonValue> = if base_val.is_object() {
        from_value(base_val).unwrap_or_default()
    } else {
        MergeableMap::default()
    };

    match map_with_strategy(base_map, next_map, &()) {
        Ok(Some(merged)) => {
            *base = to_value(merged.into_map()).unwrap_or(Value::Null);
        }
        _ => {
            // Shouldn't happen, but don't lose data.
            *base = next;
        }
    }
}

/// Merge as `MergeableVec<JsonValue>`.
///
/// Note: the default for unannotated arrays in `JsonValue` is *replace*, not
/// append (which is `MergeableVec`'s default). Plain arrays are wrapped in a
/// `Merged` with `Replace` strategy to get the right behavior.
fn merge_as_vec(base: &mut Value, next: Value) {
    let next_vec = if next.is_array() {
        // Plain array (no annotation): default to replace.
        MergeableVec::Merged(MergedVec {
            value: from_value(next.clone()).unwrap_or_default(),
            strategy: Some(MergedVecStrategy::Replace),
            discard_when_merged: false,
        })
    } else {
        // Annotation object: let MergeableVec deserializer handle it.
        let Ok(v) = from_value(next.clone()) else {
            *base = next;
            return;
        };
        v
    };

    let base_val = std::mem::replace(base, Value::Null);
    let base_vec: MergeableVec<JsonValue> = if base_val.is_array() {
        from_value(base_val).unwrap_or_default()
    } else {
        MergeableVec::default()
    };

    match vec_with_strategy(base_vec, next_vec, &()) {
        Ok(Some(merged)) => {
            *base = to_value(Vec::from(merged)).unwrap_or(Value::Null);
        }
        _ => {
            *base = next;
        }
    }
}

/// Merge as [`PartialMergeableString`].
fn merge_as_string(base: &mut Value, next: Value) {
    let Ok(next_str) = from_value::<PartialMergeableString>(next.clone()) else {
        *base = next;
        return;
    };

    let base_str = PartialMergeableString::String(base.as_str().unwrap_or_default().to_owned());

    match string_with_strategy(base_str, next_str, &()) {
        Ok(Some(merged)) => {
            // Extract the string from whichever variant we got.
            let s = match merged {
                PartialMergeableString::String(s) => s,
                PartialMergeableString::Merged(m) => m.value.unwrap_or_default(),
            };
            *base = Value::String(s);
        }
        _ => {
            *base = next;
        }
    }
}

/// Strip merge annotations from a value tree for finalize.
///
/// After all merges, values inserted via the Vacant path (single config layer)
/// may still contain the annotation wrapper. This walks the tree and unwraps
/// them by attempting deserialization as each `Mergeable*` type.
fn strip_annotations(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Does this look like an annotation? (has value + strategy keys)
            if map.contains_key("value") && map.contains_key("strategy") {
                // Try MergeableMap.
                if let Ok(MergeableMap::<Value>::Merged(m)) = from_value(Value::Object(map.clone()))
                {
                    *value = to_value(m.value).unwrap_or(Value::Null);
                    strip_annotations(value);
                    return;
                }
                // Try MergeableVec.
                if let Ok(MergeableVec::<Value>::Merged(v)) = from_value(Value::Object(map.clone()))
                {
                    *value = to_value(v.value).unwrap_or(Value::Null);
                    strip_annotations(value);
                    return;
                }
                // Try MergeableString.
                if let Ok(PartialMergeableString::Merged(s)) =
                    from_value(Value::Object(map.clone()))
                {
                    *value = Value::String(s.value.unwrap_or_default());
                    return;
                }
            }

            for v in map.values_mut() {
                strip_annotations(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                strip_annotations(v);
            }
        }
        _ => {}
    }
}

impl AssignKeyValue for JsonValue {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        // Leaf field, assign directly.
        if kv.key_string().is_empty() {
            let next = kv.value.into_value();
            merge_values(self, next);

            return Ok(());
        }

        let Some(key) = kv.trim_prefix_any() else {
            return missing_key(&kv);
        };

        if !self.is_object() {
            **self = Value::Object(Map::default());
        }

        let child = self
            .as_object_mut()
            .expect("just ensured object")
            .entry(&key)
            .or_insert(Value::Null);

        let mut wrapper = Self(child.take());
        wrapper.assign(kv)?;
        *child = wrapper.0;

        Ok(())
    }
}

impl Schematic for JsonValue {
    fn schema_name() -> Option<String> {
        Some("JsonValue".to_owned())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.set_type(SchemaType::Unknown);
        schema.build()
    }
}

impl Config for JsonValue {
    type Partial = Self;

    fn from_partial(partial: Self, _fields: Vec<String>) -> Result<Self, ConfigError> {
        Ok(partial)
    }
}

impl PartialConfig for JsonValue {
    type Context = ();

    fn default_values(_context: &()) -> Result<Option<Self>, ConfigError> {
        Ok(None)
    }

    fn env_values() -> Result<Option<Self>, ConfigError> {
        Ok(None)
    }

    fn finalize(mut self, _context: &()) -> Result<Self, ConfigError> {
        strip_annotations(&mut self.0);
        Ok(self)
    }

    fn merge(&mut self, _context: &(), next: Self) -> Result<(), ConfigError> {
        merge_values(&mut self.0, next.0);
        Ok(())
    }

    fn empty() -> Self {
        Self(Value::Null)
    }

    fn is_empty(&self) -> bool {
        self.0.is_null()
    }
}

#[cfg(test)]
#[path = "json_value_tests.rs"]
mod tests;
