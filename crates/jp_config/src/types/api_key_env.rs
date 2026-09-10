//! Where a provider reads its API keys from.

use std::{collections::BTreeMap, convert::Infallible, fmt, str::FromStr};

use schematic::{Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The environment variable, or variables, holding a provider's API keys.
///
/// One key needs no name:
///
/// ```toml
/// api_key_env = "ANTHROPIC_API_KEY"
/// ```
///
/// Several are named, and the name is what an `auth` entry selects with
/// `api_key:<name>`:
///
/// ```toml
/// api_key_env = { work = "WORK_ANTHROPIC_KEY", personal = "PERSONAL_ANTHROPIC_KEY" }
/// ```
///
/// Names where a key is read from; never holds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyEnv {
    /// One key, in the named variable.
    One(String),

    /// Several keys, each with a name and its own variable.
    Many(BTreeMap<String, String>),
}

/// Why [`ApiKeyEnv::variable`] could not name a variable.
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

    /// A map with nothing in it.
    #[error("no API keys are configured")]
    Empty,
}

/// Render the available names for an error, or nothing when there are none.
fn render_available(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }

    format!(" (configured: {})", names.join(", "))
}

impl ApiKeyEnv {
    /// The variable holding the key `name` asks for, or the sole key when
    /// `name` is `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is unknown, when `None` has several keys
    /// to choose between, or when none are configured.
    pub fn variable(&self, name: Option<&str>) -> Result<&str, ApiKeyEnvError> {
        match (self, name) {
            (Self::One(variable), None) => Ok(variable),

            // A lone variable answers to no name.
            (Self::One(_), Some(name)) => Err(ApiKeyEnvError::Unknown {
                name: name.to_owned(),
                available: vec![],
            }),

            (Self::Many(keys), Some(name)) => {
                keys.get(name)
                    .map(String::as_str)
                    .ok_or_else(|| ApiKeyEnvError::Unknown {
                        name: name.to_owned(),
                        available: keys.keys().cloned().collect(),
                    })
            }

            (Self::Many(keys), None) => match keys.len() {
                0 => Err(ApiKeyEnvError::Empty),
                1 => keys
                    .values()
                    .next()
                    .map(String::as_str)
                    .ok_or(ApiKeyEnvError::Empty),
                _ => Err(ApiKeyEnvError::Ambiguous {
                    available: keys.keys().cloned().collect(),
                }),
            },
        }
    }

    /// Every name [`Self::variable`] accepts, empty for a lone variable.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::One(_) => vec![],
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

    /// Read the single-variable form; the map form arrives as an object and
    /// never reaches here.
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
            Self::Many(keys) => keys.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ApiKeyEnv {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(BTreeMap<String, String>),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::One(variable) => Self::One(variable),
            Raw::Many(keys) => Self::Many(keys),
        })
    }
}

impl Schematic for ApiKeyEnv {
    fn schema_name() -> Option<String> {
        Some("ApiKeyEnv".into())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.string_default()
    }
}

#[cfg(test)]
#[path = "api_key_env_tests.rs"]
mod tests;
