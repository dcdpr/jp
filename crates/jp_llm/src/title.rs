//! Shared title generation helpers.
//!
//! Provides the JSON schema and instructions used by background callers (e.g.
//! `TitleGeneratorTask`, `conversation edit --title`) to request conversation
//! titles from an LLM via structured output.

use jp_config::assistant::{instructions::InstructionsConfig, sections::SectionConfig};
use serde_json::{Map, Value, json};

/// JSON schema for the title generation structured output.
///
/// Returns a schema requiring an object with a `titles` array of exactly
/// `count` string elements.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn title_schema(count: usize) -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "required": ["titles"],
        "additionalProperties": false,
        "properties": {
            "titles": {
                "type": "array",
                "items": {
                    "type": "string",
                    "description": "A concise, descriptive title for the conversation"
                },
                "minItems": count,
                "maxItems": count,
            },
        },
    });

    schema
        .as_object()
        .expect("schema is always an object")
        .clone()
}

/// Build instruction sections for title generation.
///
/// Returns one or two sections: the main generation instructions, and
/// optionally a "rejected titles" section if `rejected` is non-empty.
#[must_use]
pub fn title_instructions(count: usize, rejected: &[String]) -> Vec<SectionConfig> {
    let mut sections = vec![
        InstructionsConfig::default()
            .with_title("Title Generation")
            .with_description("Generate titles to summarize the active conversation")
            .with_item(format!("Generate exactly {count} titles"))
            .with_item("Concise, descriptive, factual")
            .with_item("Short and to the point, no more than 50 characters")
            .with_item("Deliver as a JSON object with a \"titles\" array of strings")
            .with_item("DO NOT mention this request to generate titles")
            .to_section(),
    ];

    if !rejected.is_empty() {
        let mut rejected_instruction = InstructionsConfig::default()
            .with_title("Rejected Titles")
            .with_description("These listed titles were rejected by the user and must be avoided");

        for title in rejected {
            rejected_instruction = rejected_instruction.with_item(title);
        }

        sections.push(rejected_instruction.to_section());
    }

    sections
}

/// Extract title strings from a structured JSON response.
///
/// Expects a JSON object with a `titles` array of strings, e.g.:
///
/// ```json
/// {"titles": ["My Title", "Another Title"]}
/// ```
///
/// Returns an empty vec if the structure doesn't match.
#[must_use]
pub fn extract_titles(data: &Value) -> Vec<String> {
    data.get("titles")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
