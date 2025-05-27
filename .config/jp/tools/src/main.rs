use std::{path::PathBuf, sync::Mutex};

use duct::cmd;
use indoc::formatdoc;
use mcp_attr::{
    server::{mcp_server, serve_stdio, McpServer, RequestContext},
    Result,
};
use serde_json::{from_str, Value};

#[tokio::main]
#[expect(clippy::result_large_err)]
async fn main() -> Result<()> {
    serve_stdio(ToolsServer(Mutex::new(Data { _todo: () }))).await?;
    Ok(())
}

#[expect(dead_code)]
struct ToolsServer(Mutex<Data>);

struct Data {
    _todo: (),
}

#[mcp_server]
impl McpServer for ToolsServer {
    #[tool]
    /// Execute all unit and integration tests and build examples of the
    /// project.
    async fn cargo_test(
        &self,
        /// Package to run tests for.
        package: Option<String>,
        /// If specified, only run tests containing this string in their names.
        testname: Option<String>,
        ctx: &RequestContext,
    ) -> Result<String> {
        #[derive(serde::Serialize)]
        struct TestResult {
            total_tests: usize,
            failures: Vec<TestFailure>,
        }

        #[derive(serde::Serialize)]
        struct TestFailure {
            #[serde(rename = "crate")]
            krate: String,
            path: String,
            stdout: String,
        }

        let root = ctx
            .roots_list()
            .await?
            .iter()
            .find_map(|v| {
                v.name
                    .as_ref()
                    .is_some_and(|v| v.as_str() == "project")
                    .then(|| v.to_file_path())
                    .flatten()
            })
            .unwrap_or(PathBuf::from("."));

        let test_name = testname.unwrap_or_default();
        let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
        let result = cmd!(
            "cargo",
            "nextest",
            "run",
            package,
            // Twice to silence Cargo completely.
            "--cargo-quiet",
            "--cargo-quiet",
            // Run all tests, even if one fails.
            "--no-fail-fast",
            // Dense output for better LLM readability.
            "--hide-progress-bar",
            "--final-status-level=none",
            "--status-level=fail",
            // JSON output to be parsed by the tool.
            "--message-format=libtest-json-plus",
            test_name
        )
        .dir(root)
        .env("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1")
        .env("RUST_BACKTRACE", "1")
        .unchecked()
        .read()?;

        let mut total_tests = 0;
        let mut failures = vec![];
        for l in result.lines().filter_map(|s| from_str::<Value>(s).ok()) {
            if l.get("type").and_then(Value::as_str) == Some("test") {
                total_tests += 1;
            } else {
                continue;
            }
            if l.get("event").and_then(Value::as_str) != Some("failed") {
                continue;
            }
            let Some(name) = l.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(stdout) = l.get("stdout").and_then(Value::as_str) else {
                continue;
            };

            let (krate, path) = name.split_once("$").unwrap_or(("", name));
            let krate = krate.split_once("::").unwrap_or((krate, "")).0;

            failures.push(TestFailure {
                krate: krate.to_owned(),
                path: path.to_owned(),
                stdout: stdout.to_owned(),
            });
        }

        Ok(formatdoc! {"
            The test run was completed successfully:

            {}
        ", to_xml(TestResult {
            total_tests,
            failures,
        })})
    }
}

fn to_xml<T: serde::Serialize>(failures: T) -> String {
    let mut buffer = String::new();
    let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
    serializer.indent(' ', 2);
    match failures.serialize(serializer) {
        Ok(_) => buffer,
        Err(error) => format!("<error>{error}</error>"),
    }
}
