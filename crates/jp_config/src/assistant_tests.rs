use pretty_assertions::assert_eq;
use schematic::PartialConfig as _;
use serde_json::{Value, json};
use test_log::test;

use super::*;
use crate::{
    model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
    types::{
        string::{MergedStringSeparator, MergedStringStrategy, PartialMergedString},
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
            strategy: None,
            dedup: None,
            value: vec![PartialInstructionsConfig {
                title: Some("foo".into()),
                ..Default::default()
            }],
            // NOTE: this is `true`, because the default value for this
            // field is `true`, and when we do `try_from_cli` we trigger
            // `try_vec_of_nested` on `&mut [PartialInstructionsConfig]`,
            // NOT on the `MergeableVec<PartialInstructionsConfig>`. This
            // means `discard_when_merged` is left untouched. This is
            // *correct*, but it might be confusing in some cases, so we
            // might want to change this in the future.
            discard_when_merged: true,
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
            strategy: None,
            dedup: None,
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
            discard_when_merged: true,
        })
    );

    let kv = KvAssignment::try_from_cli("instructions+", "baz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.instructions,
        MergeableVec::Merged(MergedVec {
            strategy: None,
            dedup: None,
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
            discard_when_merged: true,
        })
    );

    let kv = KvAssignment::try_from_cli("instructions", "qux").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.instructions,
        MergeableVec::Merged(MergedVec {
            strategy: None,
            dedup: None,
            value: vec![PartialInstructionsConfig {
                title: Some("qux".into()),
                ..Default::default()
            }],
            discard_when_merged: true,
        })
    );

    let kv = KvAssignment::try_from_cli("instructions.0.title", "boop").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.instructions,
        MergeableVec::Merged(MergedVec {
            strategy: None,
            dedup: None,
            value: vec![PartialInstructionsConfig {
                title: Some("boop".into()),
                ..Default::default()
            }],
            discard_when_merged: true,
        })
    );

    let kv = KvAssignment::try_from_cli("instructions.0:", r#"{"title":"quux","items":["one"]}"#)
        .unwrap();

    p.assign(kv).unwrap();
    assert_eq!(
        p.instructions,
        MergeableVec::Merged(MergedVec {
            strategy: None,
            dedup: None,
            value: vec![PartialInstructionsConfig {
                title: Some("quux".into()),
                items: Some(vec!["one".into()]),
                ..Default::default()
            }],
            discard_when_merged: true,
        })
    );

    let kv = KvAssignment::try_from_cli("instructions.0.items.0", "two").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.instructions,
        MergeableVec::Merged(MergedVec {
            strategy: None,
            dedup: None,
            value: vec![PartialInstructionsConfig {
                title: Some("quux".into()),
                items: Some(vec!["two".into()]),
                ..Default::default()
            }],
            discard_when_merged: true,
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
            separator: None,
            discard_when_merged: None,
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
            separator: None,
            discard_when_merged: None,
        }))
    );

    let kv = KvAssignment::try_from_cli(
        "system_prompt:",
        r#"{"value":"foo", "strategy":"append", "separator":"space"}"#,
    )
    .unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.system_prompt,
        Some(PartialMergeableString::Merged(PartialMergedString {
            value: Some("foo".into()),
            strategy: Some(MergedStringStrategy::Append),
            separator: Some(MergedStringSeparator::Space),
            discard_when_merged: None,
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
                instructions: vec![
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
                ]
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
                    strategy: Some(MergedVecStrategy::Append),
                    ..Default::default()
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
                    strategy: Some(MergedVecStrategy::Append),
                    ..Default::default()
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
                    "strategy": "append",
                    "separator": "paragraph",
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
                    strategy: Some(MergedStringStrategy::Append),
                    separator: Some(MergedStringSeparator::Paragraph),
                    discard_when_merged: None,
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
                    strategy: Some(MergedVecStrategy::Append),
                    ..Default::default()
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
