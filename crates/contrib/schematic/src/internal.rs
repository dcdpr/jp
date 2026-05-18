use std::str::FromStr;

use schematic_types::Schema;

use crate::config::{ConfigError, HandlerError, MergeError, MergeResult, PartialConfig};

// Handles T and Option<T> values
pub fn handle_default_result<T, E: std::error::Error>(
    result: Result<T, E>,
) -> Result<T, ConfigError> {
    result.map_err(|error| ConfigError::InvalidDefaultValue(error.to_string()))
}

/// Records that an env-derived value was set, returning the value unchanged.
#[cfg(feature = "env")]
pub fn track_env<T>(value: Option<T>, tracker: &mut bool) -> Option<T> {
    value.inspect(|_| {
        *tracker = true;
    })
}

/// Records that a nested env-derived value was set, returning it unchanged.
#[cfg(feature = "env")]
pub fn track_env_nested<T>(value: T, tracker: &mut bool) -> T {
    *tracker = true;
    value
}

#[cfg(feature = "env")]
pub fn default_env_value<T: FromStr>(key: &str) -> crate::config::ParseEnvResult<T> {
    parse_env_value(key, |value| parse_value(value).map(|v| Some(v)))
}

#[cfg(feature = "env")]
pub fn parse_env_value<T>(
    key: &str,
    parser: impl Fn(&str) -> crate::config::ParseEnvResult<T>,
) -> crate::config::ParseEnvResult<T> {
    if let Ok(value) = std::env::var(key) {
        return parser(&value)
            .map_err(|error| HandlerError(format!("Invalid environment variable {key}. {error}")));
    }

    Ok(None)
}

pub fn parse_value<T: FromStr, V: AsRef<str>>(value: V) -> Result<T, HandlerError> {
    let value = value.as_ref();

    value.parse::<T>().map_err(|_| {
        HandlerError(format!(
            "Failed to parse \"{value}\" into the correct type."
        ))
    })
}

#[allow(clippy::unnecessary_unwrap)]
pub fn merge_setting<T, C>(
    prev: Option<T>,
    next: Option<T>,
    context: &C,
    merger: impl Fn(T, T, &C) -> MergeResult<T>,
) -> MergeResult<T> {
    if prev.is_some() && next.is_some() {
        merger(prev.unwrap(), next.unwrap(), context)
    } else if next.is_some() {
        Ok(next)
    } else {
        Ok(prev)
    }
}

#[allow(clippy::unnecessary_unwrap)]
pub fn merge_nested_map_setting<T: Default, C>(
    prev: T,
    next: T,
    context: &C,
    merger: impl Fn(T, T, &C) -> MergeResult<T>,
) -> Result<T, MergeError> {
    match merger(prev, next, context)? {
        Some(value) => Ok(value),
        None => Ok(T::default()),
    }
}

#[allow(clippy::unnecessary_unwrap)]
pub fn merge_nested_optional_setting<T: PartialConfig>(
    prev: Option<T>,
    next: Option<T>,
    context: &T::Context,
) -> MergeResult<T> {
    if prev.is_some() && next.is_some() {
        let mut nested = prev.unwrap();

        nested
            .merge(context, next.unwrap())
            .map_err(|error| MergeError(error.to_string()))?;

        Ok(Some(nested))
    } else if next.is_some() {
        Ok(next)
    } else {
        Ok(prev)
    }
}

#[allow(clippy::unnecessary_unwrap)]
pub fn merge_nested_setting<T: PartialConfig>(
    prev: T,
    next: T,
    context: &T::Context,
) -> std::result::Result<T, MergeError> {
    if !prev.is_empty() && !next.is_empty() {
        let mut nested = prev;

        nested
            .merge(context, next)
            .map_err(|error| MergeError(error.to_string()))?;

        Ok(nested)
    } else if !next.is_empty() {
        Ok(next)
    } else {
        Ok(prev)
    }
}

pub fn partialize_schema(schema: &mut Schema, force_partial: bool) {
    use schematic_types::SchemaType;

    let mut update_name = |update: bool| {
        if update
            && let Some(name) = &schema.name
            && !name.starts_with("Partial")
        {
            schema.name = Some(format!("Partial{name}"));
        }
    };

    match &mut schema.ty {
        SchemaType::Array(inner) => {
            partialize_schema(&mut inner.items_type, false);
        }
        SchemaType::Object(inner) => {
            partialize_schema(&mut inner.key_type, false);
            partialize_schema(&mut inner.value_type, false);
        }
        SchemaType::Struct(inner) => {
            if inner.partial || force_partial {
                update_name(true);

                for field in inner.fields.values_mut() {
                    field.optional = true;
                    field.nullable = true;
                    field.schema.nullify();

                    partialize_schema(&mut field.schema, true);
                }
            } else {
                for field in inner.fields.values_mut() {
                    partialize_schema(&mut field.schema, false);
                }
            }
        }
        SchemaType::Tuple(inner) => {
            for item in &mut inner.items_types {
                partialize_schema(item, false);
            }
        }
        SchemaType::Union(inner) => {
            update_name(inner.partial || force_partial);

            for variant in &mut inner.variants_types {
                partialize_schema(variant, false);
            }
        }
        _ => {}
    }
}
