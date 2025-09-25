//! LLM model configuration.

pub mod id;
pub mod parameters;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    model::{
        id::{ModelIdConfig, PartialModelIdConfig},
        parameters::{ParametersConfig, PartialParametersConfig},
    },
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ModelConfig {
    /// The model ID.
    #[setting(nested)]
    pub id: ModelIdConfig,

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

#[cfg(test)]
mod tests {

    use std::assert_matches::assert_matches;

    use schematic::PartialConfig as _;
    use serde_json::Value;

    use super::*;
    use crate::model::{
        id::{PartialModelIdConfig, ProviderId},
        parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort},
    };

    #[test]
    fn test_model_config_id() {
        let mut p = PartialModelConfig::default_values(&()).unwrap().unwrap();

        assert!(p.id.name.is_none() && p.id.provider.is_none());

        let kv = KvAssignment::try_from_cli(":", r#"{"id":{"provider":"google","name":"foo"}}"#)
            .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.id, PartialModelIdConfig {
            provider: Some(ProviderId::Google),
            name: Some("foo".parse().unwrap()),
        });

        let kv =
            KvAssignment::try_from_cli("id:", r#"{"provider":"google","name":"bar"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.id, PartialModelIdConfig {
            provider: Some(ProviderId::Google),
            name: Some("bar".parse().unwrap()),
        });

        let kv = KvAssignment::try_from_cli("id.provider", "openai").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.id, PartialModelIdConfig {
            provider: Some(ProviderId::Openai),
            name: Some("bar".parse().unwrap()),
        });

        let kv = KvAssignment::try_from_cli("id.name", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.id, PartialModelIdConfig {
            provider: Some(ProviderId::Openai),
            name: Some("baz".parse().unwrap()),
        });

        let kv = KvAssignment::try_from_cli("id", "google/gemini").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.id, PartialModelIdConfig {
            provider: Some(ProviderId::Google),
            name: Some("gemini".parse().unwrap()),
        });
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
}
