//! LLM model configuration.

pub mod id;
pub mod parameters;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::PartialConfigDelta,
    model::{
        id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig},
        parameters::{ParametersConfig, PartialParametersConfig},
    },
    partial::ToPartial,
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ModelConfig {
    /// The model ID.
    #[setting(nested)]
    pub id: ModelIdOrAliasConfig,

    /// The model parameters.
    #[setting(nested)]
    pub parameters: ParametersConfig,
}

impl AssignKeyValue for PartialModelConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("id") => self.id.assign(kv)?,
            _ if kv.p("parameters") => self.parameters.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialModelConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            id: self.id.delta(next.id),
            parameters: self.parameters.delta(next.parameters),
        }
    }
}

impl ToPartial for ModelConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            id: self.id.to_partial(),
            parameters: self.parameters.to_partial(),
        }
    }
}

#[cfg(test)]
mod tests {

    use assert_matches::assert_matches;
    use schematic::PartialConfig as _;
    use serde_json::{json, Value};

    use super::*;
    use crate::model::{
        id::{PartialModelIdConfig, ProviderId},
        parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort},
    };

    #[test]
    fn test_model_config_id() {
        let mut p = PartialModelConfig::default_values(&()).unwrap().unwrap();

        assert!(p.id.is_empty());

        let kv = KvAssignment::try_from_cli(":", r#"{"id":{"provider":"google","name":"foo"}}"#)
            .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.id,
            PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some("foo".parse().unwrap()),
            }
            .into()
        );

        let kv =
            KvAssignment::try_from_cli("id:", r#"{"provider":"google","name":"bar"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.id,
            PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some("bar".parse().unwrap()),
            }
            .into()
        );

        let kv = KvAssignment::try_from_cli("id.provider", "openai").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.id,
            PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some("bar".parse().unwrap()),
            }
            .into()
        );

        let kv = KvAssignment::try_from_cli("id.name", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.id,
            PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some("baz".parse().unwrap()),
            }
            .into()
        );

        let kv = KvAssignment::try_from_cli("id", "google/gemini").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.id,
            PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some("gemini".parse().unwrap()),
            }
            .into()
        );
    }

    #[test]
    fn test_model_config_parameters() {
        let mut p = PartialModelConfig::default_values(&()).unwrap().unwrap();

        assert!(p.parameters.max_tokens.is_none());
        assert!(p.parameters.reasoning.is_none());
        assert!(p.parameters.temperature.is_none());
        assert!(p.parameters.top_p.is_none());
        assert!(p.parameters.top_k.is_none());
        assert!(p.parameters.stop_words.is_none());

        let kv = KvAssignment::try_from_cli("parameters.max_tokens", "42").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.parameters.max_tokens, Some(42));

        let kv = KvAssignment::try_from_cli("parameters.reasoning.effort", "low").unwrap();
        p.assign(kv).unwrap();
        assert_matches!(
            p.parameters.reasoning,
            Some(PartialReasoningConfig::Custom(
                PartialCustomReasoningConfig {
                    effort: Some(ReasoningEffort::Low),
                    ..
                }
            ))
        );

        let kv = KvAssignment::try_from_cli("parameters.temperature", "0.42").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.parameters.temperature, Some(0.42));

        let kv = KvAssignment::try_from_cli("parameters.top_p", "0.42").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.parameters.top_p, Some(0.42));

        let kv = KvAssignment::try_from_cli("parameters.top_k", "42").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.parameters.top_k, Some(42));

        let kv = KvAssignment::try_from_cli("parameters.stop_words", "foo,bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.parameters.stop_words,
            Some(vec!["foo".into(), "bar".into()])
        );

        let kv = KvAssignment::try_from_cli("parameters:", r#"{"max_tokens":42,"reasoning":{"effort":"low"},"temperature":0.42,"top_p":0.42,"top_k":42,"stop_words":["foo","bar"]}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.parameters.max_tokens, Some(42));
        assert_matches!(
            p.parameters.reasoning,
            Some(PartialReasoningConfig::Custom(
                PartialCustomReasoningConfig {
                    effort: Some(ReasoningEffort::Low),
                    ..
                }
            ))
        );
        assert_eq!(p.parameters.temperature, Some(0.42));
        assert_eq!(p.parameters.top_p, Some(0.42));
        assert_eq!(p.parameters.top_k, Some(42));
        assert_eq!(
            p.parameters.stop_words,
            Some(vec!["foo".into(), "bar".into()])
        );

        let kv = KvAssignment::try_from_cli("parameters:", r#"{"reasoning":"off"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_matches!(p.parameters.reasoning, Some(PartialReasoningConfig::Off));

        let kv = KvAssignment::try_from_cli("parameters.foo", "bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.parameters.other.and_then(|v| v.get("foo").cloned()),
            Some(Value::String("bar".into()))
        );
    }

    #[test]
    fn test_model_config_deserialize() {
        struct TestCase {
            data: Value,
            expected: Result<PartialModelConfig, String>,
        }

        let cases = vec![
            TestCase {
                data: json!({
                    "id": { "provider": "ollama", "name": "bar" },
                }),
                expected: Ok(PartialModelConfig {
                    id: PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
                        provider: Some(ProviderId::Ollama),
                        name: "bar".parse().ok(),
                    }),
                    ..Default::default()
                }),
            },
            TestCase {
                data: json!({
                    "id": "llamacpp/bar",
                }),
                expected: Ok(PartialModelConfig {
                    id: PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
                        provider: Some(ProviderId::Llamacpp),
                        name: "bar".parse().ok(),
                    }),
                    ..Default::default()
                }),
            },
            TestCase {
                data: json!({
                    "id": "llamabar",
                }),
                expected: Ok(PartialModelConfig {
                    id: PartialModelIdOrAliasConfig::Alias("llamabar".into()),
                    ..Default::default()
                }),
            },
            TestCase {
                data: json!({
                    "id": "foo/bar",
                }),
                // TODO: See if we can get a better error message.
                // expected: Err("Alias must not be empty and must not contain '/'.".into()),
                expected: Err("data did not match any variant of untagged enum \
                               PartialModelIdOrAliasConfig"
                    .into()),
            },
        ];

        for TestCase { data, expected } in cases {
            let result = serde_json::from_value::<PartialModelConfig>(data);
            assert_eq!(result.map_err(|e| e.to_string()), expected);
        }
    }
}
