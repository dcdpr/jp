use std::error::Error;

use jp_conversation::{message::Messages, thread::ThreadBuilder};
use serde_json::Value;

use crate::query::StructuredQuery;

pub fn titles(
    count: usize,
    messages: Messages,
    rejected: &[String],
) -> Result<StructuredQuery, Box<dyn Error + Send + Sync>> {
    let schema = schemars::json_schema!({
        "type": "object",
        "description": format!("Generate {count} concise, descriptive factual titles for this conversation."),
        "required": ["titles"],
        "additionalProperties": false,
        "properties": {
            "titles": {
                "type": "array",
                "items": {
                    "type": "string",
                    "description": "A concise, descriptive title for the conversation"
                },
                // TODO: Not supported by OpenAI. Investigate if other providers
                // support this, if not, remove it.
                //
                // Detailed explanation:
                //
                // By default, OpenAI ignores JSON schema properties it does not
                // yet support. However, if you set the `strict` property to
                // `true`, OpenAI will return an error for unsupported
                // properties.
                //
                // Setting `strict` to `true` is encouraged by OpenAI, as it
                // (significantly) increases the likelihood of getting a
                // response that matches the schema.
                //
                // We have logic that retries the request with `strict` set to
                // `false` if the first request returns an error, so technically
                // we can keep these two properties, and it will work fine, but
                // we might get sub-par responses, and we add the overhead of an
                // extra request.
                //
                // "minItems": count,
                // "maxItems": count,
            },
        },
    });

    let mut message = indoc::formatdoc!(
        "Generate {count} concise, descriptive, factual titles for this conversation. Try to keep \
         them short and to the point, no more than 50 characters.

         DO NOT generate titles about the request to generate titles!"
    );

    if !rejected.is_empty() {
        let rejected = rejected.join("\n -");
        message =
            format!("{message}\n\nThe following titles were rejected by the user:\n\n- {rejected}");
    }

    let mapping = |value: &mut Value| value.get_mut("titles").map(Value::take);
    let thread = ThreadBuilder::default()
        .with_history(messages)
        .with_message(message)
        .build()?;

    Ok(StructuredQuery::new(schema, thread).with_mapping(mapping))
}
