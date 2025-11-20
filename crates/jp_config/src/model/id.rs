//! LLM model ID configuration.

mod alias;

use std::{fmt, str::FromStr};

use indexmap::IndexMap;
use jp_id::{
    Id,
    parts::{GlobalId, TargetId, Variant},
};
use schematic::{Config, ConfigEnum, Schematic};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Either a [`ModelIdConfig`] or a named alias for one.
#[derive(Debug, Clone, Config)]
#[config(serde(untagged))]
pub enum ModelIdOrAliasConfig {
    /// A model ID configuration.
    #[setting(nested, empty)]
    Id(ModelIdConfig),

    /// A named alias for a model ID configuration.
    ///
    /// The matching [`ModelIdConfig`] be fetched using
    /// [`LlmProviderConfig::aliases`].
    ///
    /// [`LlmProviderConfig::aliases`]: crate::providers::llm::LlmProviderConfig::aliases
    #[setting(with = "alias")]
    Alias(String),
}

impl AssignKeyValue for PartialModelIdOrAliasConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "provider" | "name" => match self {
                Self::Id(id) => id.assign(kv)?,
                Self::Alias(_) => return missing_key(&kv),
            },
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialModelIdOrAliasConfig {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Id(prev), Self::Id(next)) => Self::Id(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for ModelIdOrAliasConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Id(id) => Self::Partial::Id(id.to_partial()),
            Self::Alias(alias) => Self::Partial::Alias(alias.clone()),
        }
    }
}

impl FromStr for ModelIdOrAliasConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ModelIdConfig::from_str(s)
            .map(Self::Id)
            .or_else(|_| Ok(Self::Alias(s.to_owned())))
    }
}

impl From<&str> for PartialModelIdOrAliasConfig {
    fn from(s: &str) -> Self {
        PartialModelIdConfig::from_str(s).map_or_else(|_| Self::Alias(s.to_owned()), Self::Id)
    }
}

impl FromStr for PartialModelIdOrAliasConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PartialModelIdConfig::from_str(s)
            .map(Self::Id)
            .or_else(|_| Ok(Self::Alias(s.to_owned())))
    }
}

impl fmt::Display for ModelIdOrAliasConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => id.fmt(f),
            Self::Alias(alias) => f.write_str(alias),
        }
    }
}

impl fmt::Display for PartialModelIdOrAliasConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => id.fmt(f),
            Self::Alias(alias) => f.write_str(alias),
        }
    }
}

impl ModelIdOrAliasConfig {
    /// Finalize the model ID configuration.
    ///
    /// This will resolve to a [`ModelIdConfig`] if the configuration has one
    /// defined, has an alias that can be resolved to one, or a name that can be
    /// parsed into one.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be resolved.
    pub fn finalize(
        &self,
        aliases: &IndexMap<String, ModelIdConfig>,
    ) -> Result<ModelIdConfig, ModelIdConfigError> {
        match &self {
            Self::Id(id) => Ok(id.clone()),
            Self::Alias(alias) => aliases
                .get(alias)
                .cloned()
                .map_or_else(|| ModelIdConfig::from_str(alias), Ok),
        }
    }
}

impl PartialModelIdOrAliasConfig {
    /// See [`ModelIdOrAliasConfig::finalize`].
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be resolved.
    pub fn finalize(
        &self,
        aliases: &IndexMap<String, PartialModelIdConfig>,
    ) -> Result<PartialModelIdConfig, ModelIdConfigError> {
        match &self {
            Self::Id(id) => Ok(id.clone()),
            Self::Alias(alias) => aliases
                .get(alias)
                .cloned()
                .map_or_else(|| PartialModelIdConfig::from_str(alias), Ok),
        }
    }
}

/// Assistant-specific configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", no_deserialize_derive)]
pub struct ModelIdConfig {
    /// The provider to supply the model.
    #[setting(required)]
    pub provider: ProviderId,

    /// The actual model name.
    #[setting(required)]
    pub name: Name,
}

impl AssignKeyValue for PartialModelIdConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "provider" => self.provider = kv.try_some_from_str()?,
            "name" => self.name = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialModelIdConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            provider: delta_opt(self.provider.as_ref(), next.provider),
            name: delta_opt(self.name.as_ref(), next.name),
        }
    }
}

impl ToPartial for ModelIdConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            provider: partial_opt(&self.provider, defaults.provider),
            name: partial_opt(&self.name, defaults.name),
        }
    }
}

impl fmt::Display for ModelIdConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.name)
    }
}

impl fmt::Display for PartialModelIdConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.provider, &self.name) {
            (Some(provider), Some(name)) => write!(f, "{provider}/{name}"),
            (Some(provider), None) => write!(f, "{provider}"),
            (None, Some(name)) => write!(f, "{name}"),
            (None, None) => Ok(()),
        }
    }
}

impl<'de> Deserialize<'de> for PartialModelIdConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ModelIdConfigVisitor;

        impl<'de> Visitor<'de> for ModelIdConfigVisitor {
            type Value = PartialModelIdConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string or map")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.parse::<PartialModelIdConfig>().map_err(E::custom)
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut provider: Option<ProviderId> = None;
                let mut name: Option<Name> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "provider" => {
                            if provider.is_some() {
                                return Err(de::Error::duplicate_field("provider"));
                            }
                            provider = Some(map.next_value()?);
                        }
                        "name" => {
                            if name.is_some() {
                                return Err(de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(PartialModelIdConfig { provider, name })
            }
        }

        deserializer.deserialize_any(ModelIdConfigVisitor)
    }
}

impl From<PartialModelIdConfig> for PartialModelIdOrAliasConfig {
    fn from(v: PartialModelIdConfig) -> Self {
        Self::Id(v)
    }
}

impl TryFrom<(ProviderId, String)> for ModelIdConfig {
    type Error = ModelIdConfigError;

    fn try_from((provider, name): (ProviderId, String)) -> Result<Self, Self::Error> {
        (provider, &name).try_into()
    }
}

impl TryFrom<(ProviderId, &String)> for ModelIdConfig {
    type Error = ModelIdConfigError;

    fn try_from((provider, name): (ProviderId, &String)) -> Result<Self, Self::Error> {
        (provider, name.as_str()).try_into()
    }
}

impl TryFrom<(ProviderId, &str)> for ModelIdConfig {
    type Error = ModelIdConfigError;

    fn try_from((provider, name): (ProviderId, &str)) -> Result<Self, Self::Error> {
        Ok(Self {
            provider,
            name: name.parse()?,
        })
    }
}

/// Error when parsing `ModelIdConfig`.
#[derive(Debug, thiserror::Error)]
pub enum ModelIdConfigError {
    /// Error when parsing `ModelIdConfig` from a string.
    #[error("model ID config must match <provider>/<model>")]
    StrParse,

    /// Error when parsing `ProviderId`.
    #[error(transparent)]
    ProviderId(#[from] schematic::ConfigError),

    /// Error when parsing `ModelId`.
    #[error(transparent)]
    ModelId(#[from] ModelIdError),
}

impl FromStr for ModelIdConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, id) = s
            .split_once('/')
            .map(|(p, n)| (p.trim(), n.trim()))
            .ok_or(ModelIdConfigError::StrParse)?;

        Ok(Self {
            provider: ProviderId::from_str(provider)?,
            name: Name::from_str(id)?,
        })
    }
}

impl FromStr for PartialModelIdConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, name) = s
            .split_once('/')
            .map(|(p, n)| (p.trim(), n.trim()))
            .ok_or(ModelIdConfigError::StrParse)?;

        Ok(Self {
            provider: Some(ProviderId::from_str(provider)?),
            name: Some(Name::from_str(name)?),
        })
    }
}

/// The list of supported providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    /// Anthropic provider. See: <https://www.anthropic.com/api>.
    Anthropic,
    /// Deepseek provider. See: <https://api-docs.deepseek.com>. UNIMPLEMENTED.
    Deepseek,
    /// Google Gemini provider. See: <https://ai.google.dev/gemini-api/docs>.
    Google,
    /// Llama.cpp provider. See: <https://github.com/ggml-org/llama.cpp>.
    Llamacpp,
    /// Ollama provider. See: <https://ollama.com>.
    Ollama,
    /// Openai provider. See: <https://openai.com/api/>.
    Openai,
    /// Openrouter provider. See: <https://openrouter.io>.
    Openrouter,
    /// xAI provider. See: <https://x.ai/api>. UNIMPLEMENTED.
    Xai,
}

impl Id for ProviderId {
    fn variant() -> Variant {
        'p'.into()
    }

    fn target_id(&self) -> TargetId {
        self.to_string().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

/// A model ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(try_from = "String")]
pub struct Name(pub String);

impl std::ops::Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<String> for Name {
    type Error = ModelIdError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::from_str(id.as_str())
    }
}

impl TryFrom<&str> for Name {
    type Error = ModelIdError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::from_str(id)
    }
}

impl FromStr for Name {
    type Err = ModelIdError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        if id.is_empty()
            || id.chars().any(|c| {
                !(c.is_numeric()
                    || c.is_ascii_alphabetic()
                    || c == '-'
                    || c == '_'
                    || c == '.'
                    || c == ':'
                    || c == '/')
            })
        {
            return Err(ModelIdError);
        }

        Ok(Self(id.to_owned()))
    }
}

impl From<Name> for String {
    fn from(id: Name) -> Self {
        id.to_string()
    }
}

/// Error when parsing `ModelId`.
#[derive(Debug, thiserror::Error)]
#[error("Model ID must be [a-zA-Z0-9_-.:/]+")]
pub struct ModelIdError;

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn test_model_id_config_deserialize() {
        struct TestCase {
            data: Value,
            expected: PartialModelIdConfig,
        }

        let cases = vec![
            TestCase {
                data: json!({
                    "provider": "ollama",
                    "name": "bar",
                }),
                expected: PartialModelIdConfig {
                    provider: Some(ProviderId::Ollama),
                    name: "bar".parse().ok(),
                },
            },
            TestCase {
                data: json!("llamacpp/bar"),
                expected: PartialModelIdConfig {
                    provider: Some(ProviderId::Llamacpp),
                    name: "bar".parse().ok(),
                },
            },
        ];

        for TestCase { data, expected } in cases {
            let result = serde_json::from_value::<PartialModelIdConfig>(data);
            assert_eq!(result.unwrap(), expected);
        }
    }
}
