use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use indexmap::IndexMap;
use jp_config::providers::mcp::{McpProviderConfig, StdioConfig};
use tokio::{process::Command, runtime::Handle};

use super::{render_command, render_stderr_tail};
use crate::{Client, Error, id::McpServerId};

fn stdio_config(command: &str, optional: bool) -> McpProviderConfig {
    McpProviderConfig::Stdio(StdioConfig {
        command: PathBuf::from(command),
        arguments: vec![],
        variables: vec![],
        checksum: None,
        optional,
        startup_timeout_secs: 60,
    })
}

#[test]
fn render_command_program_only() {
    let cmd = Command::new("just");
    assert_eq!(render_command(&cmd), "just");
}

#[test]
fn render_command_program_with_args() {
    let mut cmd = Command::new("just");
    cmd.arg("serve-bookworm");
    assert_eq!(render_command(&cmd), "just serve-bookworm");
}

#[test]
fn render_command_program_with_multiple_args() {
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--release")
        .arg("--package")
        .arg("bookworm");
    assert_eq!(
        render_command(&cmd),
        "cargo build --release --package bookworm"
    );
}

#[test]
fn render_stderr_tail_empty_returns_empty_string() {
    let buffer = Arc::new(Mutex::new(VecDeque::<String>::new()));
    assert_eq!(render_stderr_tail(&buffer), "");
}

#[test]
fn render_stderr_tail_formats_lines_with_header_and_indent() {
    let mut buf = VecDeque::new();
    buf.push_back("error: multiple workspace roots".to_owned());
    buf.push_back("  /path/a".to_owned());
    buf.push_back("  /path/b".to_owned());
    let buffer = Arc::new(Mutex::new(buf));

    let expected = "\nstderr:\n  error: multiple workspace roots\n    /path/a\n    /path/b";
    assert_eq!(render_stderr_tail(&buffer), expected);
}

#[test]
fn initialize_error_display_includes_command_and_stderr() {
    let error = Error::InitializeError {
        cmd: "just serve-bookworm".to_owned(),
        error: "connection closed: initialize response".to_owned(),
        stderr: "\nstderr:\n  error: multiple workspace roots".to_owned(),
    };

    let rendered = error.to_string();
    assert!(
        rendered.contains("just serve-bookworm"),
        "expected command in error: {rendered}"
    );
    assert!(
        rendered.contains("connection closed: initialize response"),
        "expected underlying error in: {rendered}"
    );
    assert!(
        rendered.contains("error: multiple workspace roots"),
        "expected stderr tail in: {rendered}"
    );
}

#[test]
fn initialize_error_display_omits_stderr_section_when_empty() {
    let error = Error::InitializeError {
        cmd: "just".to_owned(),
        error: "boom".to_owned(),
        stderr: String::new(),
    };

    assert_eq!(
        error.to_string(),
        "Server initialization error: just, error: boom"
    );
}

#[test]
fn initialize_timeout_display_includes_timeout_command_and_stderr() {
    let error = Error::InitializeTimeout {
        cmd: "just serve-bookworm".to_owned(),
        timeout_secs: 60,
        stderr: "\nstderr:\n  Compiling bookworm v0.1.0".to_owned(),
    };

    assert_eq!(
        error.to_string(),
        "Server initialization timed out after 60s: just serve-bookworm\nstderr:\n  Compiling \
         bookworm v0.1.0"
    );
}

#[test]
fn initialize_timeout_display_omits_stderr_section_when_empty() {
    let error = Error::InitializeTimeout {
        cmd: "just".to_owned(),
        timeout_secs: 5,
        stderr: String::new(),
    };

    assert_eq!(
        error.to_string(),
        "Server initialization timed out after 5s: just"
    );
}

// A child that writes to stderr and then hangs without ever speaking MCP
// forces the startup-timeout path; the resulting error must carry the
// configured deadline and the captured stderr tail.
#[cfg(unix)]
#[tokio::test]
async fn startup_timeout_attaches_stderr_tail() {
    let config = McpProviderConfig::Stdio(StdioConfig {
        command: PathBuf::from("sh"),
        arguments: vec!["-c".to_owned(), "echo compiling >&2; sleep 30".to_owned()],
        variables: vec![],
        checksum: None,
        optional: false,
        startup_timeout_secs: 1,
    });

    let error = Client::create_client(&McpServerId::new("slow"), &config)
        .await
        .expect_err("a server that never completes the handshake must time out");

    match error {
        Error::InitializeTimeout {
            cmd,
            timeout_secs,
            stderr,
        } => {
            assert_eq!(cmd, "sh -c echo compiling >&2; sleep 30");
            assert_eq!(timeout_secs, 1);
            assert_eq!(stderr, "\nstderr:\n  compiling");
        }
        other => panic!("expected InitializeTimeout, got: {other:?}"),
    }
}

// `startup_timeout_secs = 0` disables the timeout: startup must run through the
// un-timed serve path. A child that exits immediately without speaking MCP
// fails that path quickly, so the error is `InitializeError`, never the
// `InitializeTimeout` that a zero-duration `tokio::time::timeout` would raise.
#[cfg(unix)]
#[tokio::test]
async fn zero_startup_timeout_disables_timeout() {
    let config = McpProviderConfig::Stdio(StdioConfig {
        command: PathBuf::from("sh"),
        arguments: vec!["-c".to_owned(), "exit 0".to_owned()],
        variables: vec![],
        checksum: None,
        optional: false,
        startup_timeout_secs: 0,
    });

    let error = Client::create_client(&McpServerId::new("instant"), &config)
        .await
        .expect_err("a child that exits without a handshake fails initialization");

    assert!(
        matches!(error, Error::InitializeError { .. }),
        "zero timeout must take the un-timed serve path, got: {error:?}"
    );
}

// Use a binary path that cannot exist on any sane system. This drives
// `create_client` into a `CannotSpawnProcess` error path without depending on
// any specific environment behavior.
const MISSING_BINARY: &str = "/nonexistent/jp-mcp-test/missing-binary";

#[tokio::test]
async fn optional_server_failure_is_tolerated() {
    let server_name = "missing".to_owned();
    let mut providers = IndexMap::new();
    providers.insert(server_name.clone(), stdio_config(MISSING_BINARY, true));

    let mut client = Client::new(providers);
    let server_id = McpServerId::new(&server_name);

    let mut startup = client
        .run_services(HashSet::from([server_id.clone()]), Handle::current())
        .await
        .expect("run_services should not fail for optional servers");

    assert_eq!(startup.pending, vec![server_id.clone()]);

    while let Some(joined) = startup.joins.join_next().await {
        let completed = joined
            .expect("task did not panic")
            .expect("optional failure is swallowed inside the task");
        assert_eq!(completed, server_id);
    }

    assert!(
        !client.is_running(&server_id).await,
        "failed optional server must not be registered as running"
    );
}

#[tokio::test]
async fn required_server_failure_propagates() {
    let server_name = "missing".to_owned();
    let mut providers = IndexMap::new();
    providers.insert(server_name.clone(), stdio_config(MISSING_BINARY, false));

    let mut client = Client::new(providers);
    let server_id = McpServerId::new(&server_name);

    let mut startup = client
        .run_services(HashSet::from([server_id.clone()]), Handle::current())
        .await
        .expect("run_services itself returns Ok; per-task results carry the error");

    assert_eq!(startup.pending, vec![server_id.clone()]);

    let mut saw_error = false;
    while let Some(joined) = startup.joins.join_next().await {
        let task_result = joined.expect("task did not panic");
        if task_result.is_err() {
            saw_error = true;
        }
    }

    assert!(
        saw_error,
        "required server failure must surface as a task error"
    );
    assert!(
        !client.is_running(&server_id).await,
        "failed required server is also not registered as running"
    );
}
