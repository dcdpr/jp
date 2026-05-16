//! Typewriter effect styling configuration.

use std::{fmt, str::FromStr, time::Duration};

use humantime::{format_duration, parse_duration};
use schematic::{Config, Schema, SchemaBuilder, Schematic};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Typewriter style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TypewriterConfig {
    /// Delay between printing characters.
    ///
    /// Accepts any [`humantime`]-compatible duration string, e.g. `"3ms"`,
    /// `"500us"`, `"1s"`. Use `"0s"` to disable.
    ///
    /// The default is `3ms`.
    #[setting(default = "3ms")]
    pub text_delay: DelayDuration,

    /// Delay between printing code-block characters.
    ///
    /// Accepts the same formats as `text_delay`. The default is `500us`.
    #[setting(default = "500us")]
    pub code_delay: DelayDuration,

    /// Maximum latency the typewriter is allowed to fall behind the source.
    ///
    /// When set to a non-zero value, the typewriter acts as a bounded-
    /// latency controller: the effective per-character delay shrinks below
    /// `text_delay`/`code_delay` as the queue of pending characters grows,
    /// keeping printed output within `max_latency` of what the source has
    /// already emitted. With a fast provider (e.g. Cerebras) this prevents
    /// the typewriter from falling many seconds behind. When the source
    /// stops emitting, the controller switches to drain mode and stops
    /// slowing back down as the queue empties.
    ///
    /// Accepts the same formats as `text_delay`. The default is `0s`, which
    /// disables the controller and keeps the static `text_delay`/
    /// `code_delay` behavior.
    #[setting(default = "0s")]
    pub max_latency: DelayDuration,
}

impl AssignKeyValue for PartialTypewriterConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "text_delay" => self.text_delay = kv.try_some_from_str()?,
            "code_delay" => self.code_delay = kv.try_some_from_str()?,
            "max_latency" => self.max_latency = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTypewriterConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            text_delay: delta_opt(self.text_delay.as_ref(), next.text_delay),
            code_delay: delta_opt(self.code_delay.as_ref(), next.code_delay),
            max_latency: delta_opt(self.max_latency.as_ref(), next.max_latency),
        }
    }
}

impl FillDefaults for PartialTypewriterConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            text_delay: self.text_delay.or(defaults.text_delay),
            code_delay: self.code_delay.or(defaults.code_delay),
            max_latency: self.max_latency.or(defaults.max_latency),
        }
    }
}

impl ToPartial for TypewriterConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            text_delay: partial_opt(&self.text_delay, defaults.text_delay),
            code_delay: partial_opt(&self.code_delay, defaults.code_delay),
            max_latency: partial_opt(&self.max_latency, defaults.max_latency),
        }
    }
}

/// Typewriter delay duration.
///
/// Parses and serializes using the [`humantime`] format, e.g. `"3ms"`,
/// `"500us"`, `"1s"`, `"0s"` to disable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DelayDuration(Duration);

impl DelayDuration {
    /// Sets the delay to `0`.
    #[must_use]
    pub const fn instant() -> Self {
        Self(Duration::from_secs(0))
    }
}

impl FromStr for DelayDuration {
    type Err = humantime::DurationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_duration(s).map(Self)
    }
}

impl TryFrom<&str> for DelayDuration {
    type Error = humantime::DurationError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for DelayDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_duration(self.0).fmt(f)
    }
}

impl Serialize for DelayDuration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_duration(self.0))
    }
}

impl<'de> Deserialize<'de> for DelayDuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DelayVisitor;

        impl<'de> Visitor<'de> for DelayVisitor {
            type Value = DelayDuration;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a humantime duration string (e.g. \"3ms\") or a legacy {secs, nanos} object",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<DelayDuration, E> {
                v.parse().map_err(de::Error::custom)
            }

            // Backwards-compat: stored conversations from before the
            // humantime change serialized `DelayDuration` using `Duration`'s
            // default `{secs, nanos}` representation. Accept that shape on
            // read so existing conversations keep loading; new writes still
            // go out as a humantime string via `Serialize`.
            //
            // This is a tactical patch; the general fix for stored-config
            // schema drift is tracked in RFD D27.
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<DelayDuration, M::Error> {
                let mut secs: u64 = 0;
                let mut nanos: u32 = 0;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "secs" => secs = map.next_value()?,
                        "nanos" => nanos = map.next_value()?,
                        _ => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(DelayDuration(Duration::new(secs, nanos)))
            }
        }

        deserializer.deserialize_any(DelayVisitor)
    }
}

impl Schematic for DelayDuration {
    fn schema_name() -> Option<String> {
        Some("DelayDuration".into())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        let mut schema = schema.string(schematic::schema::StringType::default());
        schema.set_description(
            "Typewriter delay duration in humantime format (e.g. \"3ms\", \"500us\", \"1s\").",
        );
        schema
    }
}

impl From<DelayDuration> for Duration {
    fn from(delay: DelayDuration) -> Self {
        delay.0
    }
}

#[cfg(test)]
#[path = "typewriter_tests.rs"]
mod tests;
