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

    let format_parameters = context.format_parameters;
    let name = tool.name.clone();
    match run(context, tool).await {
        Ok(Outcome::Success { content }) if format_parameters => println!("{content}"),
        Ok(outcome) => match serde_json::to_string(&outcome) {
            Ok(content) => println!("{content}"),
            Err(error) => handle_error(&error, &name),
        },
        Err(error) => handle_error(error.as_ref(), &name),
    }
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

fn handle_error(error: &dyn std::error::Error, name: &str) {
    let mut sources = vec![];
    let mut source = Some(error);
    while let Some(error) = source {
        sources.push(format!("{error:#}"));
        source = error.source();
    }

    println!(
        "```json\n{:#}\n```",
        json!({
            "error": format!("An error occurred while running the '{name}' tool."),
            "trace": sources,
        })
    );
}
