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
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::tool_choice::ToolChoice,
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    internal::merge::{string_with_strategy, vec_with_strategy},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
    types::{
        string::{MergeableString, PartialMergeableString},
        vec::{MergeableVec, MergedVec, MergedVecStrategy},
    },
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AssistantConfig {
    /// Optional name of the assistant.
    pub name: Option<String>,

    /// The system prompt to use for the assistant.
    #[setting(nested, default = "You are a helpful assistant.", merge = string_with_strategy)]
    pub system_prompt: Option<MergeableString>,

    /// A list of instructions for the assistant.
    #[setting(nested, default = default_instructions, merge = vec_with_strategy)]
    pub instructions: MergeableVec<InstructionsConfig>,

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
            "system_prompt" => self.system_prompt = kv.try_some_object_or_from_str()?,
            _ if kv.p("instructions") => kv.try_vec_of_nested(self.instructions.as_mut())?,
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
            system_prompt: delta_opt_partial(self.system_prompt.as_ref(), next.system_prompt),
            instructions: {
                next.instructions
                    .into_iter()
                    .filter(|v| !self.instructions.contains(v))
                    .collect::<Vec<_>>()
                    .into()
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
            system_prompt: partial_opt_config(self.system_prompt.as_ref(), defaults.system_prompt),
            instructions: self.instructions.to_partial(),
            tool_choice: partial_opt(&self.tool_choice, defaults.tool_choice),
            model: self.model.to_partial(),
        }
    }
}

/// The default instructions for the assistant.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
fn default_instructions(_: &()) -> TransformResult<MergeableVec<PartialInstructionsConfig>> {
    Ok(MergeableVec::Merged(MergedVec {
        strategy: MergedVecStrategy::Replace,
        value: vec![PartialInstructionsConfig {
            title: Some("How to respond to the user".into()),
            items: Some(vec![
                "Be concise".into(),
                "Use simple sentences. But feel free to use technical jargon.".into(),
                "Do NOT overexplain basic concepts. Assume the user is technically proficient."
                    .into(),
                "AVOID flattering, corporate-ish or marketing language. Maintain a neutral \
                 viewpoint."
                    .into(),
                "AVOID vague and / or generic claims which may seem correct but are not \
                 substantiated by the context."
                    .into(),
            ]),
            ..Default::default()
        }],
    }))
}

#[cfg(test)]
mod tests {
    use schematic::PartialConfig as _;
    use serde_json::{Value, json};
    use test_log::test;

    use super::*;
    use crate::{
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        types::{
            string::{MergedStringStrategy, PartialMergedString},
            vec::{MergedVec, MergedVecStrategy},
        },
    };

    #[test]
    fn test_assistant_config_instructions() {
        let mut p = PartialAssistantConfig::default_values(&())
            .unwrap()
            .unwrap();

        assert!(p.instructions[0].title.as_deref() == Some("How to respond to the user"));

        let kv = KvAssignment::try_from_cli("instructions:", r#"[{"title":"foo"}]"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![PartialInstructionsConfig {
                    title: Some("foo".into()),
                    ..Default::default()
                }],
            })
        );

        let kv = KvAssignment::try_from_cli(
            "instructions+:",
            r#"[{"title":"bar", "description":"hello"}]"#,
        )
        .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![
                    PartialInstructionsConfig {
                        title: Some("foo".into()),
                        ..Default::default()
                    },
                    PartialInstructionsConfig {
                        title: Some("bar".into()),
                        description: Some("hello".into()),
                        ..Default::default()
                    }
                ],
            })
        );

        let kv = KvAssignment::try_from_cli("instructions+", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![
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
                ],
            })
        );

        let kv = KvAssignment::try_from_cli("instructions", "qux").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![PartialInstructionsConfig {
                    title: Some("qux".into()),
                    ..Default::default()
                }],
            })
        );

        let kv = KvAssignment::try_from_cli("instructions.0.title", "boop").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![PartialInstructionsConfig {
                    title: Some("boop".into()),
                    ..Default::default()
                }],
            })
        );

        let kv =
            KvAssignment::try_from_cli("instructions.0:", r#"{"title":"quux","items":["one"]}"#)
                .unwrap();

        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![PartialInstructionsConfig {
                    title: Some("quux".into()),
                    items: Some(vec!["one".into()]),
                    ..Default::default()
                }],
            })
        );

        let kv = KvAssignment::try_from_cli("instructions.0.items.0", "two").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.instructions,
            MergeableVec::Merged(MergedVec {
                strategy: MergedVecStrategy::Replace,
                value: vec![PartialInstructionsConfig {
                    title: Some("quux".into()),
                    items: Some(vec!["two".into()]),
                    ..Default::default()
                }],
            })
        );

        let kv = KvAssignment::try_from_cli("instructions:", r#"[{title:"foo"}]"#).unwrap_err();
        assert_eq!(
            &kv.to_string(),
            "instructions: key must be a string at line 1 column 3"
        );

        let kv = KvAssignment::try_from_cli("system_prompt", "foo").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.system_prompt,
            Some(PartialMergeableString::String("foo".into()))
        );

        let kv = KvAssignment::try_from_cli("system_prompt:", r#"{"value":"foo"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.system_prompt,
            Some(PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".into()),
                strategy: None,
            }))
        );

        let kv =
            KvAssignment::try_from_cli("system_prompt:", r#"{"value":"foo", "strategy":"append"}"#)
                .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.system_prompt,
            Some(PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".into()),
                strategy: Some(MergedStringStrategy::Append),
            }))
        );

        let kv = KvAssignment::try_from_cli(
            "system_prompt:",
            r#"{"value":"foo", "strategy":"append_space"}"#,
        )
        .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.system_prompt,
            Some(PartialMergeableString::Merged(PartialMergedString {
                value: Some("foo".into()),
                strategy: Some(MergedStringStrategy::AppendSpace),
            }))
        );
    }

    #[test]
    fn test_assistant_config_model() {
        let mut p = PartialAssistantConfig::default_values(&())
            .unwrap()
            .unwrap();

        assert!(p.model.id.is_empty());

        let kv =
            KvAssignment::try_from_cli("model:", r#"{"id":{"provider":"anthropic","name":"foo"}}"#)
                .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.model, PartialModelConfig {
            id: PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some("foo".parse().unwrap()),
            }),
            ..Default::default()
        });
    }

    #[test]
    fn test_assistant_config_instructions_merge() {
        struct TestCase {
            prev: PartialAssistantConfig,
            next: PartialAssistantConfig,
            expected: PartialAssistantConfig,
        }

        let cases = vec![
            TestCase {
                prev: PartialAssistantConfig {
                    instructions: vec![PartialInstructionsConfig {
                        title: Some("foo".into()),
                        description: None,
                        position: None,
                        items: None,
                        examples: vec![],
                    }]
                    .into(),
                    ..Default::default()
                },
                next: PartialAssistantConfig {
                    instructions: vec![PartialInstructionsConfig {
                        title: Some("bar".into()),
                        description: None,
                        position: None,
                        items: None,
                        examples: vec![],
                    }]
                    .into(),
                    ..Default::default()
                },
                expected: PartialAssistantConfig {
                    instructions: vec![PartialInstructionsConfig {
                        title: Some("bar".into()),
                        description: None,
                        position: None,
                        items: None,
                        examples: vec![],
                    }]
                    .into(),
                    ..Default::default()
                },
            },
            TestCase {
                prev: PartialAssistantConfig {
                    instructions: vec![PartialInstructionsConfig {
                        title: Some("foo".into()),
                        description: None,
                        position: None,
                        items: None,
                        examples: vec![],
                    }]
                    .into(),
                    ..Default::default()
                },
                next: PartialAssistantConfig {
                    instructions: MergedVec {
                        value: vec![PartialInstructionsConfig {
                            title: Some("bar".into()),
                            description: None,
                            position: None,
                            items: None,
                            examples: vec![],
                        }],
                        strategy: MergedVecStrategy::Append,
                    }
                    .into(),
                    ..Default::default()
                },
                expected: PartialAssistantConfig {
                    instructions: MergedVec {
                        value: vec![
                            PartialInstructionsConfig {
                                title: Some("foo".into()),
                                description: None,
                                position: None,
                                items: None,
                                examples: vec![],
                            },
                            PartialInstructionsConfig {
                                title: Some("bar".into()),
                                description: None,
                                position: None,
                                items: None,
                                examples: vec![],
                            },
                        ],
                        strategy: MergedVecStrategy::Append,
                    }
                    .into(),
                    ..Default::default()
                },
            },
        ];

        for TestCase {
            mut prev,
            next,
            expected,
        } in cases
        {
            prev.merge(&(), next).unwrap();
            assert_eq!(prev, expected);
        }
    }

    #[test]
    fn test_assistant_config_deserialize() {
        struct TestCase {
            data: Value,
            expected: PartialAssistantConfig,
        }

        let cases = vec![
            TestCase {
                data: json!({
                    "system_prompt": "foo",
                    "instructions": [
                        {
                            "title": "foo",
                            "description": "bar",
                        },
                        {
                            "title": "bar",
                            "description": "baz",
                        }
                    ]
                }),
                expected: PartialAssistantConfig {
                    system_prompt: Some(PartialMergeableString::String("foo".into())),
                    instructions: vec![
                        PartialInstructionsConfig {
                            title: Some("foo".into()),
                            description: Some("bar".into()),
                            position: None,
                            items: None,
                            examples: vec![],
                        },
                        PartialInstructionsConfig {
                            title: Some("bar".into()),
                            description: Some("baz".into()),
                            position: None,
                            items: None,
                            examples: vec![],
                        },
                    ]
                    .into(),
                    ..Default::default()
                },
            },
            TestCase {
                data: json!({
                    "system_prompt": {
                        "value": "foo",
                        "strategy": "append_paragraph"
                    },
                    "instructions": {
                        "value": [
                            {
                                "title": "foo",
                                "description": "bar",
                            },
                            {
                                "title": "bar",
                                "description": "baz",
                            }
                        ],
                        "strategy": "append"
                    }
                }),
                expected: PartialAssistantConfig {
                    system_prompt: Some(PartialMergeableString::Merged(PartialMergedString {
                        value: Some("foo".into()),
                        strategy: Some(MergedStringStrategy::AppendParagraph),
                    })),
                    instructions: MergedVec {
                        value: vec![
                            PartialInstructionsConfig {
                                title: Some("foo".into()),
                                description: Some("bar".into()),
                                position: None,
                                items: None,
                                examples: vec![],
                            },
                            PartialInstructionsConfig {
                                title: Some("bar".into()),
                                description: Some("baz".into()),
                                position: None,
                                items: None,
                                examples: vec![],
                            },
                        ],
                        strategy: MergedVecStrategy::Append,
                    }
                    .into(),
                    ..Default::default()
                },
            },
        ];

        for TestCase { data, expected } in cases {
            let result = serde_json::from_value::<PartialAssistantConfig>(data);
            assert_eq!(result.unwrap(), expected);
        }
    }
}
