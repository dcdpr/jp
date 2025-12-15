use std::error::Error;

use jp_config::assistant::instructions::InstructionsConfig;
use jp_conversation::{ConversationStream, thread::ThreadBuilder};
use serde_json::Value;

use crate::query::StructuredQuery;

pub fn titles(
    count: usize,
    events: ConversationStream,
    rejected: &[String],
) -> Result<StructuredQuery, Box<dyn Error + Send + Sync>> {
    let schema = schemars::json_schema!({
        "type": "object",
        "description": format!("Provide {count} concise, descriptive factual titles for this conversation."),
        "required": ["titles"],
        "additionalProperties": false,
        "properties": {
            "titles": {
                "type": "array",
                "items": {
                    "type": "string",
                    "description": "A concise, descriptive title for the conversation"
                },
            },
        },
    });

    // The validator schema is more strict than the schema we use to generate
    // the titles, because not all providers support the full JSON schema
    // feature-set.
    let validator = schemars::json_schema!({
        "type": "object",
        "required": ["titles"],
        "additionalProperties": false,
        "properties": {
            "titles": {
                "type": "array",
                "items": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 50,
                },
                "minItems": count,
                "maxItems": count,
            },
        },
    });

    let mut instructions = vec![
        InstructionsConfig::default()
            .with_title("Title Generation")
            .with_description("Generate titles to summarize the active conversation")
            .with_item(format!("Generate exactly {count} titles"))
            .with_item("Concise, descriptive, factual")
            .with_item("Short and to the point, no more than 50 characters")
            .with_item("Deliver as a JSON array of strings")
            .with_item("DO NOT mention this request to generate titles"),
    ];

    if !rejected.is_empty() {
        let mut instruction = InstructionsConfig::default()
            .with_title("Rejected Titles")
            .with_description("These listed titles were rejected by the user and must be avoided");

        for title in rejected {
            instruction = instruction.with_item(title);
        }

        instructions.push(instruction);
    }

    let mapping = |value: &mut Value| value.get_mut("titles").map(Value::take);
    let thread = ThreadBuilder::default()
        .with_events(events)
        .with_instructions(instructions)
        .build()?;

    Ok(StructuredQuery::new(schema, thread)
        .with_mapping(mapping)
        .with_schema_validator(validator))
}
