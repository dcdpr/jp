use std::env;

use jp_tool::{Context, Outcome};
use serde_json::{from_str, json};
use tools::{Tool, run};

#[tokio::main]
async fn main() {
    let context = match input::<Context>(1, "context") {
        Ok(workspace) => workspace,
        Err(error) => return println!("{error}"),
    };

    let tool: Tool = match input(2, "tool") {
        Ok(tool) => tool,
        Err(error) => return println!("{error}"),
    };

    let name = tool.name.clone();
    let result = run(context, tool)
        .await
        .unwrap_or_else(|error| error_outcome(error.as_ref(), &name));

    let json = serde_json::to_string(&result).unwrap_or_else(|error| {
        format!(r#"{{"type":"error","message":"Unable to serialize result: {error}","trace":[],"transient":false}}"#)
    });

    println!("{json}");
}

fn input<T: serde::de::DeserializeOwned>(index: usize, name: &str) -> Result<T, String> {
    env::args()
        .nth(index)
        .ok_or(json!({
            "error": format!("Missing {name} input argument at index {index}."),
        }))
        .and_then(|arg| {
            from_str::<T>(&arg).map_err(|error| {
                json!({
                    "error": format!("Unable to parse {name} input argument at index {index}."),
                    "cause": format!("{error:#}"),
                })
            })
        })
        .map_err(|error| format!("```json\n{error:#}\n```"))
}

fn error_outcome(error: &dyn std::error::Error, name: &str) -> Outcome {
    let mut trace = vec![];
    let mut source = Some(error);
    while let Some(error) = source {
        trace.push(format!("{error:#}"));
        source = error.source();
    }

    Outcome::Error {
        message: format!("An error occurred while running the '{name}' tool."),
        trace,
        transient: true,
    }
}
