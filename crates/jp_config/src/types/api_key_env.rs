//! Where a provider reads its API keys from.

use std::{collections::BTreeMap, convert::Infallible, fmt, slice, str::FromStr};

use schematic::{Schema, SchemaBuilder, Schematic, schema::UnionType};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The environment variable, or variables, holding a provider's API keys.
///
/// One key needs no name:
///
/// ```toml
/// api_key_env = "ANTHROPIC_API_KEY"
/// ```
///
/// A list is still one key, read from the first variable that holds a non-empty
/// value:
///
/// ```toml
/// api_key_env = ["WORK_ANTHROPIC_KEY", "ANTHROPIC_API_KEY"]
/// ```
///
/// Several keys are named, and the name is what an `auth` entry selects with
/// `api_key:<name>`:
///
/// ```toml
/// api_key_env = { work = "WORK_ANTHROPIC_KEY", personal = "PERSONAL_ANTHROPIC_KEY" }
/// ```
///
/// A named key can be a list too, read the same way as an unnamed one:
///
/// ```toml
/// api_key_env = { work = ["WORK_ANTHROPIC_KEY", "ANTHROPIC_API_KEY"] }
/// ```
///
/// Names where a key is read from; never holds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyEnv {
    /// One key, in the named variable.
    One(String),

    /// One key, in the first of these variables that holds a value.
    FirstOf(Vec<String>),

    /// Several keys, each with a name and its own variables.
    Many(BTreeMap<String, KeyVariables>),
}

/// Where one named key is read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyVariables {
    /// The named variable.
    One(String),

    /// The first of these variables that holds a value.
    FirstOf(Vec<String>),
}

impl KeyVariables {
    /// The variables to read, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::One(variable) => slice::from_ref(variable),
            Self::FirstOf(variables) => variables,
        }
    }
}

impl From<&str> for KeyVariables {
    fn from(variable: &str) -> Self {
        Self::One(variable.to_owned())
    }
}

impl fmt::Display for KeyVariables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One(variable) => f.write_str(variable),
            Self::FirstOf(variables) => write!(f, "[{}]", variables.join(", ")),
        }
    }
}

impl Schematic for KeyVariables {
    /// A variable name, or a list of them.
    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.union(UnionType::new_any([
            schema.infer::<String>(),
            schema.infer::<Vec<String>>(),
        ]))
    }
}

/// Why [`ApiKeyEnv::variables`] could not name a variable.
#[derive(Debug, thiserror::Error)]
pub enum ApiKeyEnvError {
    /// A name no key answers to.
    #[error("no API key named `{name}` is configured{}", render_available(.available))]
    Unknown {
        /// The name that was asked for.
        name: String,

        /// The names that are configured.
        available: Vec<String>,
    },

    /// No name given, with more than one key to choose between.
    #[error(
        "`api_key` is ambiguous: several keys are configured ({}); name one with \
         `api_key:<name>`",
        .available.join(", ")
    )]
    Ambiguous {
        /// Every name the bare entry could have meant.
        available: Vec<String>,
    },

    /// A list or map with nothing in it.
    #[error("no API keys are configured")]
    Empty,

    /// A named key whose list of variables is empty.
    #[error("API key `{name}` names no environment variables")]
    NoVariables {
        /// The key with nothing to read.
        name: String,
    },
}

/// Render the available names for an error, or nothing when there are none.
fn render_available(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }

    format!(" (configured: {})", names.join(", "))
}

impl ApiKeyEnv {
    /// The variables that may hold the key `name` asks for, or the sole key
    /// when `name` is `None`.
    ///
    /// The caller reads them in order and uses the first that holds a key.
    /// Only a list, unnamed or named, yields more than one variable.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is unknown, when `None` has several keys
    /// to choose between, or when none are configured.
    pub fn variables(&self, name: Option<&str>) -> Result<&[String], ApiKeyEnvError> {
        match (self, name) {
            (Self::One(variable), None) => Ok(slice::from_ref(variable)),
            (Self::FirstOf(variables), None) if variables.is_empty() => Err(ApiKeyEnvError::Empty),
            (Self::FirstOf(variables), None) => Ok(variables),

            // A lone variable, or a list of places to read one key from,
            // answers to no name.
            (Self::One(_) | Self::FirstOf(_), Some(name)) => Err(ApiKeyEnvError::Unknown {
                name: name.to_owned(),
                available: vec![],
            }),

            (Self::Many(keys), Some(name)) => {
                let variables = keys.get(name).ok_or_else(|| ApiKeyEnvError::Unknown {
                    name: name.to_owned(),
                    available: keys.keys().cloned().collect(),
                })?;

                named_variables(name, variables)
            }

            (Self::Many(keys), None) => match keys.len() {
                0 => Err(ApiKeyEnvError::Empty),
                1 => keys
                    .iter()
                    .next()
                    .ok_or(ApiKeyEnvError::Empty)
                    .and_then(|(name, variables)| named_variables(name, variables)),
                _ => Err(ApiKeyEnvError::Ambiguous {
                    available: keys.keys().cloned().collect(),
                }),
            },
        }
    }

    /// Every name [`Self::variables`] accepts, empty unless keys are named.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::One(_) | Self::FirstOf(_) => vec![],
            Self::Many(keys) => keys.keys().map(String::as_str).collect(),
        }
    }
}

impl Default for ApiKeyEnv {
    fn default() -> Self {
        Self::One(String::new())
    }
}

impl FromStr for ApiKeyEnv {
    type Err = Infallible;

    /// Read the single-variable form; the list and map forms arrive as JSON and
    /// never reach here.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::One(s.to_owned()))
    }
}

impl From<String> for ApiKeyEnv {
    fn from(variable: String) -> Self {
        Self::One(variable)
    }
}

impl From<&str> for ApiKeyEnv {
    fn from(variable: &str) -> Self {
        Self::One(variable.to_owned())
    }
}

impl fmt::Display for ApiKeyEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One(variable) => f.write_str(variable),
            Self::FirstOf(variables) => write!(f, "[{}]", variables.join(", ")),
            Self::Many(keys) => {
                let keys: Vec<_> = keys
                    .iter()
                    .map(|(name, variable)| format!("{name} = {variable}"))
                    .collect();

                write!(f, "{{ {} }}", keys.join(", "))
            }
        }
    }
}

impl Serialize for ApiKeyEnv {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::One(variable) => variable.serialize(serializer),
            Self::FirstOf(variables) => variables.serialize(serializer),
            Self::Many(keys) => keys.serialize(serializer),
        }
    }
}

/// The variables a named key reads, rejecting a key with none.
fn named_variables<'a>(
    name: &str,
    variables: &'a KeyVariables,
) -> Result<&'a [String], ApiKeyEnvError> {
    match variables.as_slice() {
        [] => Err(ApiKeyEnvError::NoVariables {
            name: name.to_owned(),
        }),
        variables => Ok(variables),
    }
}

impl<'de> Deserialize<'de> for ApiKeyEnv {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            FirstOf(Vec<String>),
            Many(BTreeMap<String, KeyVariables>),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::One(variable) => Self::One(variable),
            Raw::FirstOf(variables) => Self::FirstOf(variables),
            Raw::Many(keys) => Self::Many(keys),
        })
    }
}

impl Schematic for ApiKeyEnv {
    fn schema_name() -> Option<String> {
        Some("ApiKeyEnv".into())
    }

    /// Any form: a variable name, a list of variable names, or a map from key
    /// name to either of those.
    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.union(UnionType::new_any([
            schema.infer::<String>(),
            schema.infer::<Vec<String>>(),
            schema.infer::<BTreeMap<String, KeyVariables>>(),
        ]))
    }
}

#[cfg(test)]
#[path = "api_key_env_tests.rs"]
mod tests;
