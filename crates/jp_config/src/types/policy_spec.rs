//! A compaction policy paired with the options that qualify when it applies.
//!
//! A policy says *what* to do to an item; the options attached here say *which
//! items* it reaches.
//! Today the only option is `over`, a size threshold:
//!
//! ```toml
//! [[conversation.compaction.rules]]
//! # Strip every reasoning block in range.
//! reasoning = "strip"
//! # Strip only the tool responses that are actually large.
//! tool_calls = { policy = "strip-responses", over = "1MB" }
//! ```
//!
//! The bare form and the table form mean the same thing when no option is set,
//! and a spec without options serializes back out as the bare form.

use std::{fmt, str::FromStr};

use schematic::{Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use super::byte_size::ByteSize;
use crate::BoxedError;

/// A compaction policy plus the options qualifying which items it applies to.
///
/// `over` limits the policy to items whose decoded content exceeds the given
/// size.
/// Unset means the policy applies to every item in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicySpec<P> {
    /// What to do to the items this spec covers.
    pub policy: P,

    /// Apply the policy only to items larger than this.
    ///
    /// The comparison is strict: `over = "1MB"` leaves an item of exactly 1 MB
    /// alone.
    pub over: Option<ByteSize>,
}

impl<P> PolicySpec<P> {
    /// A spec that applies its policy to every item in range.
    pub const fn new(policy: P) -> Self {
        Self { policy, over: None }
    }

    /// A spec that applies its policy only to items larger than `over`.
    pub const fn over(policy: P, over: ByteSize) -> Self {
        Self {
            policy,
            over: Some(over),
        }
    }

    /// Whether an item of `size` bytes is large enough for the policy to apply.
    ///
    /// Always true when no threshold is set.
    #[must_use]
    pub fn covers(&self, size: u64) -> bool {
        self.over
            .is_none_or(|threshold| size > threshold.as_bytes())
    }
}

impl<P> From<P> for PolicySpec<P> {
    fn from(policy: P) -> Self {
        Self::new(policy)
    }
}

impl<P: Serialize> Serialize for PolicySpec<P> {
    /// Serialize as the bare policy when no option is set, so a stream that
    /// uses no thresholds keeps the shape older readers expect.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let Some(over) = self.over else {
            return self.policy.serialize(serializer);
        };

        // A policy that serializes to a map (one carrying its own `policy` tag,
        // such as `ToolCallPolicy`) gains `over` alongside its own fields. One
        // that serializes to a bare string is promoted to `{"policy": "..."}`
        // so the option has somewhere to live.
        let mut object = match serde_json::to_value(&self.policy).map_err(S::Error::custom)? {
            Value::Object(map) => map,
            bare => Map::from_iter([("policy".to_owned(), bare)]),
        };

        object.insert(
            "over".to_owned(),
            serde_json::to_value(over).map_err(S::Error::custom)?,
        );

        object.serialize(serializer)
    }
}

impl<'de, P: DeserializeOwned> Deserialize<'de> for PolicySpec<P> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut value = Value::deserialize(deserializer)?;

        let over = match value.as_object_mut().and_then(|map| map.remove("over")) {
            Some(raw) => Some(serde_json::from_value(raw).map_err(D::Error::custom)?),
            None => None,
        };

        // `P` may serialize either as a map carrying its own tag or as a bare
        // string that `Serialize` promoted into `{"policy": "..."}`. Which one
        // is not knowable from here, so try the whole value first and fall back
        // to the promoted string. Trying the map first matters: a tagged unit
        // variant (`{"policy": "omit"}`) is indistinguishable from a promotion
        // by shape alone, and only the map reading is correct for it.
        let policy = match serde_json::from_value::<P>(value.clone()) {
            Ok(policy) => policy,
            Err(error) => {
                let Some(map) = value.as_object() else {
                    return Err(D::Error::custom(error));
                };
                let promoted = map
                    .get("policy")
                    .cloned()
                    .ok_or_else(|| D::Error::custom(&error))?;

                // `P` did not consume the map, so `policy` is the only key it
                // can account for and anything left is a mistake. Reporting it
                // matters because the leftover is typically a misspelled option
                // (`oer = "1MB"`), and silently dropping it would apply the
                // policy to every item instead of the large ones. The rest of
                // this config tree rejects unknown fields, so this does too.
                if let Some(unknown) = map.keys().find(|key| *key != "policy") {
                    return Err(D::Error::custom(format!(
                        "unknown policy option `{unknown}`"
                    )));
                }

                serde_json::from_value(promoted).map_err(|_| D::Error::custom(error))?
            }
        };

        Ok(Self { policy, over })
    }
}

impl<P: FromStr> FromStr for PolicySpec<P>
where
    P::Err: Into<BoxedError>,
{
    type Err = BoxedError;

    /// Parse `POLICY` or `POLICY,option=value`.
    ///
    /// The separator matches the inline compaction DSL, so `--cfg
    /// ...tool_calls=strip-responses,over=1MB` and `-k 't=sres,over=1MB'` are
    /// written the same way.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(',');
        let policy = parts
            .next()
            .unwrap_or_default()
            .trim()
            .parse()
            .map_err(Into::into)?;

        let mut spec = Self::new(policy);
        for option in parts {
            let (key, value) = option.split_once('=').ok_or_else(|| {
                format!(
                    "invalid policy option `{}`: expected `key=value`",
                    option.trim()
                )
            })?;

            match key.trim() {
                "over" => spec.over = Some(value.trim().parse()?),
                other => return Err(format!("unknown policy option `{other}`").into()),
            }
        }

        Ok(spec)
    }
}

impl<P: fmt::Display> fmt::Display for PolicySpec<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.policy)?;
        if let Some(over) = self.over {
            write!(f, ",over={over}")?;
        }
        Ok(())
    }
}

impl<P: Schematic> Schematic for PolicySpec<P> {
    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        // Either the bare policy, or a table carrying it alongside the options
        // that qualify it. The table is described field by field so a schema
        // consumer still validates `policy` against `P` and still rejects an
        // unknown key, rather than falling back to "any value here".
        let table = schema.nest().structure(schematic::schema::StructType {
            required: Some(vec!["policy".to_owned()]),
            ..schematic::schema::StructType::new([
                ("policy".to_owned(), schema.infer::<P>()),
                ("over".to_owned(), schema.infer::<ByteSize>()),
            ])
        });

        schema.union(schematic::schema::UnionType {
            variants_types: vec![Box::new(schema.infer::<P>()), Box::new(table)],
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "policy_spec_tests.rs"]
mod tests;
