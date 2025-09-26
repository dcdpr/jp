//! Assistant-specific configuration for Jean-Pierre.
//!
//! These configuration options tweak the behavior of the assistant. The
//! "assistant" is defined as the technique powering the response generation
//! (typically a GPT/LLM model), with additional options built on top for
//! improved performance.

pub mod instructions;
pub mod tool_choice;

use instructions::{InstructionsConfig, PartialInstructionsConfig};
use schematic::{Config, TransformResult};

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    assistant::tool_choice::ToolChoice,
    delta::{delta_opt, PartialConfigDelta},
    model::{ModelConfig, PartialModelConfig},
    partial::{partial_opt, partial_opts, ToPartial},
};
/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct AssistantConfig {
    /// Optional name of the assistant.
    pub name: Option<String>,

    /// The system prompt to use for the assistant.
    #[setting(default = "You are a helpful assistant.")]
    pub system_prompt: String,

    /// A list of instructions for the assistant.
    #[setting(nested, default = default_instructions, merge = schematic::merge::append_vec)]
    pub instructions: Vec<InstructionsConfig>,

    /// How the assistant should choose tools to call.
    #[setting(default)]
    pub tool_choice: ToolChoice,

    /// LLM model configuration.
    #[setting(nested)]
    pub model: ModelConfig,
}

impl AssignKeyValue for PartialAssistantConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "name" => self.name = kv.try_some_string()?,
            "system_prompt" => self.system_prompt = kv.try_some_string()?,
            _ if kv.p("instructions") => kv.try_vec_of_nested(&mut self.instructions)?,
            "tool_choice" => self.tool_choice = kv.try_some_from_str()?,
            _ if kv.p("model") => self.model.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAssistantConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            name: delta_opt(self.name.as_ref(), next.name),
            system_prompt: delta_opt(self.system_prompt.as_ref(), next.system_prompt),
            instructions: {
                next.instructions
                    .into_iter()
                    .filter(|v| !self.instructions.contains(v))
                    .collect()
            },
            tool_choice: delta_opt(self.tool_choice.as_ref(), next.tool_choice),
            model: self.model.delta(next.model),
        }
    }
}

impl ToPartial for AssistantConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            name: partial_opts(self.name.as_ref(), defaults.name),
            system_prompt: partial_opt(&self.system_prompt, defaults.system_prompt),
            instructions: self
                .instructions
                .iter()
                .map(ToPartial::to_partial)
                .collect(),
            tool_choice: partial_opt(&self.tool_choice, defaults.tool_choice),
            model: self.model.to_partial(),
        }
    }
}

/// The default instructions for the assistant.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
fn default_instructions(_: &()) -> TransformResult<Vec<PartialInstructionsConfig>> {
    Ok(vec![PartialInstructionsConfig {
        title: Some("How to respond to the user".into()),
        items: Some(vec![
            "Be concise".into(),
            "Use simple sentences. But feel free to use technical jargon.".into(),
            "Do NOT overexplain basic concepts. Assume the user is technically proficient.".into(),
            "AVOID flattering, corporate-ish or marketing language. Maintain a neutral viewpoint."
                .into(),
            "AVOID vague and / or generic claims which may seem correct but are not substantiated \
             by the context."
                .into(),
        ]),
        ..Default::default()
    }])
}

#[cfg(test)]
mod tests {
    use schematic::PartialConfig as _;

    use super::*;
    use crate::model::id::{PartialModelIdConfig, ProviderId};

    #[test]
    fn test_assistant_config_instructions() {
        let mut p = PartialAssistantConfig::default_values(&())
            .unwrap()
            .unwrap();

        assert!(p.instructions[0].title.as_deref() == Some("How to respond to the user"));

        let kv = KvAssignment::try_from_cli("instructions:", r#"[{"title":"foo"}]"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![PartialInstructionsConfig {
            title: Some("foo".into()),
            ..Default::default()
        }]);

        let kv = KvAssignment::try_from_cli(
            "instructions+:",
            r#"[{"title":"bar", "description":"hello"}]"#,
        )
        .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![
            PartialInstructionsConfig {
                title: Some("foo".into()),
                ..Default::default()
            },
            PartialInstructionsConfig {
                title: Some("bar".into()),
                description: Some("hello".into()),
                ..Default::default()
            }
        ]);

        let kv = KvAssignment::try_from_cli("instructions+", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![
            PartialInstructionsConfig {
                title: Some("foo".into()),
                ..Default::default()
            },
            PartialInstructionsConfig {
                title: Some("bar".into()),
                description: Some("hello".into()),
                ..Default::default()
            },
            PartialInstructionsConfig {
                title: Some("baz".into()),
                ..Default::default()
            }
        ]);

        let kv = KvAssignment::try_from_cli("instructions", "qux").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![PartialInstructionsConfig {
            title: Some("qux".into()),
            ..Default::default()
        }]);

        let kv = KvAssignment::try_from_cli("instructions.0.title", "boop").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![PartialInstructionsConfig {
            title: Some("boop".into()),
            ..Default::default()
        }]);

        let kv =
            KvAssignment::try_from_cli("instructions.0:", r#"{"title":"quux","items":["one"]}"#)
                .unwrap();

        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![PartialInstructionsConfig {
            title: Some("quux".into()),
            items: Some(vec!["one".into()]),
            ..Default::default()
        }]);

        let kv = KvAssignment::try_from_cli("instructions.0.items.0", "two").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.instructions, vec![PartialInstructionsConfig {
            title: Some("quux".into()),
            items: Some(vec!["two".into()]),
            ..Default::default()
        }]);

        let kv = KvAssignment::try_from_cli("instructions:", r#"[{title:"foo"}]"#).unwrap_err();
        assert_eq!(
            &kv.to_string(),
            "instructions: key must be a string at line 1 column 3"
        );
    }

    #[test]
    fn test_assistant_config_model() {
        let mut p = PartialAssistantConfig::default_values(&())
            .unwrap()
            .unwrap();

        assert!(p.model.id.provider.is_none());

        let kv =
            KvAssignment::try_from_cli("model:", r#"{"id":{"provider":"anthropic","name":"foo"}}"#)
                .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.model, PartialModelConfig {
            id: PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some("foo".parse().unwrap()),
            },
            ..Default::default()
        });
    }
}
