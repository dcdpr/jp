use indexmap::IndexMap;
use jp_tool::Outcome;
use serde_json::{Value, json};

use super::*;
use crate::tool::{ParameterDocs, ToolDocs};

fn empty_tool_docs() -> ToolDocs {
    ToolDocs {
        summary: None,
        description: None,
        examples: None,
        parameters: IndexMap::new(),
    }
}

fn param_docs(
    summary: Option<&str>,
    description: Option<&str>,
    examples: Option<&str>,
) -> ParameterDocs {
    ParameterDocs {
        summary: summary.map(str::to_owned),
        description: description.map(str::to_owned),
        examples: examples.map(str::to_owned),
    }
}

fn no_answers() -> IndexMap<String, Value> {
    IndexMap::new()
}

#[test]
fn test_format_empty_docs() {
    let out = DescribeTools::format_tool_docs("my_tool", &empty_tool_docs());
    assert_eq!(out, "## my_tool\n");
}

#[test]
fn test_format_summary_only() {
    let docs = ToolDocs {
        summary: Some("A brief summary.".to_owned()),
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(out, "## my_tool\n\nA brief summary.\n");
}

#[test]
fn test_format_description_only() {
    let docs = ToolDocs {
        description: Some("Detailed description.".to_owned()),
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(out, "## my_tool\n\nDetailed description.\n");
}

#[test]
fn test_format_summary_and_description() {
    let docs = ToolDocs {
        summary: Some("Summary.".to_owned()),
        description: Some("Description.".to_owned()),
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(out, "## my_tool\n\nSummary.\n\nDescription.\n");
}

#[test]
fn test_format_all_tool_fields() {
    let docs = ToolDocs {
        summary: Some("Summary.".to_owned()),
        description: Some("Description.".to_owned()),
        examples: Some("my_tool()".to_owned()),
        parameters: IndexMap::new(),
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(
        out,
        "## my_tool\n\nSummary.\n\nDescription.\n\n### Examples\n\nmy_tool()\n"
    );
}

#[test]
fn test_format_parameter_with_description() {
    let mut parameters = IndexMap::new();
    parameters.insert(
        "input".to_owned(),
        param_docs(Some("Short summary."), Some("Long description."), None),
    );

    let docs = ToolDocs {
        parameters,
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(
        out,
        "## my_tool\n\n### Parameters\n\n#### `input`\n\nShort summary.\n\nLong description.\n"
    );
}

#[test]
fn test_format_parameter_with_examples() {
    let mut parameters = IndexMap::new();
    parameters.insert(
        "query".to_owned(),
        param_docs(None, Some("The search query."), Some("\"hello world\"")),
    );

    let docs = ToolDocs {
        parameters,
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(
        out,
        "## my_tool\n\n### Parameters\n\n#### `query`\n\nThe search query.\n\n\"hello world\"\n"
    );
}

#[test]
fn test_format_parameter_all_fields() {
    let mut parameters = IndexMap::new();
    parameters.insert(
        "path".to_owned(),
        param_docs(
            Some("File path."),
            Some("Absolute or relative path to the target file."),
            Some("\"src/main.rs\""),
        ),
    );

    let docs = ToolDocs {
        parameters,
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert_eq!(
        out,
        "## my_tool\n\n### Parameters\n\n#### `path`\n\nFile path.\n\nAbsolute or relative path \
         to the target file.\n\n\"src/main.rs\"\n"
    );
}

#[test]
fn test_format_skips_empty_parameters() {
    // is_empty() checks description and examples only, so a param with only
    // summary is still "empty" and is excluded from the output.
    let mut parameters = IndexMap::new();
    parameters.insert(
        "documented".to_owned(),
        param_docs(None, Some("Has description."), None),
    );
    parameters.insert("undocumented".to_owned(), param_docs(None, None, None));
    parameters.insert(
        "summary_only".to_owned(),
        param_docs(Some("Only a summary."), None, None),
    );

    let docs = ToolDocs {
        parameters,
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert!(
        out.contains("#### `documented`"),
        "documented param should appear"
    );
    assert!(
        !out.contains("#### `undocumented`"),
        "undocumented param should be skipped"
    );
    assert!(
        !out.contains("#### `summary_only`"),
        "summary-only param should be skipped"
    );
}

#[test]
fn test_format_no_parameters_section_when_all_params_empty() {
    // If every parameter is empty, the "### Parameters" section is omitted.
    let mut parameters = IndexMap::new();
    parameters.insert("a".to_owned(), param_docs(None, None, None));
    parameters.insert("b".to_owned(), param_docs(Some("summary only"), None, None));

    let docs = ToolDocs {
        parameters,
        ..empty_tool_docs()
    };
    let out = DescribeTools::format_tool_docs("my_tool", &docs);
    assert!(!out.contains("### Parameters"));
    assert_eq!(out, "## my_tool\n");
}

#[tokio::test]
async fn test_execute_missing_tools_argument() {
    let tool = DescribeTools::new(IndexMap::new());
    let result = tool.execute(&json!({}), &no_answers()).await;
    let Outcome::Error {
        message, transient, ..
    } = result
    else {
        panic!("expected Outcome::Error");
    };
    assert!(message.contains("`tools`"));
    assert!(!transient);
}

#[tokio::test]
async fn test_execute_tools_not_an_array() {
    let tool = DescribeTools::new(IndexMap::new());
    let result = tool
        .execute(&json!({"tools": "my_tool"}), &no_answers())
        .await;
    assert!(
        matches!(result, Outcome::Error { .. }),
        "non-array `tools` should be an error"
    );
}

#[tokio::test]
async fn test_execute_empty_tools_array() {
    let tool = DescribeTools::new(IndexMap::new());
    let result = tool.execute(&json!({"tools": []}), &no_answers()).await;
    let Outcome::Error { message, .. } = result else {
        panic!("expected Outcome::Error");
    };
    assert!(message.contains("must not be empty"));
}

#[tokio::test]
async fn test_execute_single_known_tool() {
    let mut docs = IndexMap::new();
    docs.insert("my_tool".to_owned(), ToolDocs {
        summary: Some("Tool summary.".to_owned()),
        ..empty_tool_docs()
    });

    let tool = DescribeTools::new(docs);
    let result = tool
        .execute(&json!({"tools": ["my_tool"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(content, "## my_tool\n\nTool summary.\n");
}

#[tokio::test]
async fn test_execute_known_tool_with_empty_docs() {
    let mut docs = IndexMap::new();
    docs.insert("bare_tool".to_owned(), empty_tool_docs());

    let tool = DescribeTools::new(docs);
    let result = tool
        .execute(&json!({"tools": ["bare_tool"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(content, "## bare_tool\n");
}

#[tokio::test]
async fn test_execute_multiple_known_tools_separated_by_divider() {
    let mut docs = IndexMap::new();
    docs.insert("tool_a".to_owned(), ToolDocs {
        summary: Some("A.".to_owned()),
        ..empty_tool_docs()
    });
    docs.insert("tool_b".to_owned(), ToolDocs {
        summary: Some("B.".to_owned()),
        ..empty_tool_docs()
    });

    let tool = DescribeTools::new(docs);
    let result = tool
        .execute(&json!({"tools": ["tool_a", "tool_b"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(content, "## tool_a\n\nA.\n\n---\n\n## tool_b\n\nB.\n");
}

#[tokio::test]
async fn test_execute_single_unknown_tool() {
    let tool = DescribeTools::new(IndexMap::new());
    let result = tool
        .execute(&json!({"tools": ["unknown_tool"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(
        content,
        "No additional documentation available for: unknown_tool"
    );
}

#[tokio::test]
async fn test_execute_multiple_unknown_tools() {
    let tool = DescribeTools::new(IndexMap::new());
    let result = tool
        .execute(&json!({"tools": ["foo", "bar"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(
        content,
        "No additional documentation available for: foo, bar"
    );
}

#[tokio::test]
async fn test_execute_mixed_known_and_unknown_tools() {
    let mut docs = IndexMap::new();
    docs.insert("known".to_owned(), ToolDocs {
        summary: Some("Summary.".to_owned()),
        ..empty_tool_docs()
    });

    let tool = DescribeTools::new(docs);
    let result = tool
        .execute(&json!({"tools": ["known", "unknown"]}), &no_answers())
        .await;

    let Outcome::Success { content } = result else {
        panic!("expected Outcome::Success");
    };
    assert_eq!(
        content,
        "## known\n\nSummary.\n\n---\n\nNo additional documentation available for: unknown"
    );
}
