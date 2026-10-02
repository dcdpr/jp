use std::{fs, mem, sync::Arc};

use async_trait::async_trait;
use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{CommandConfig, PartialToolConfig, ToolConfig, ToolConfigWithDefaults},
};
use jp_plugin::{PROTOCOL_VERSION, message::PathsInfo};
use jp_process::{ExitCode, MockProcessRunner, ProcessOutput};
use jp_tool::{EnvRule, Outcome, ToolDefinition, ToolDocs};
use serde_json::Map;

use super::*;
use crate::{
    Client,
    server::{
        command::sha256_file,
        testing::{echoing, no_commands},
    },
};

struct EchoArguments;

#[async_trait]
impl BuiltinTool for EchoArguments {
    async fn execute(&self, arguments: &Value, _answers: &IndexMap<String, Value>) -> Outcome {
        Outcome::Success {
            content: arguments.to_string(),
        }
    }
}

/// The pieces an [`Execution`] borrows, owned so a test can keep them alive
/// while it builds one.
struct Fixture {
    definition: ToolDefinition,
    config: ToolConfigWithDefaults,
    builtins: builtin::BuiltinExecutors,
    runner: Arc<dyn ProcessRunner>,
    command_plugins: CommandPlugins,

    /// Holds the plugin binaries [`Fixture::with_plugin`] writes.
    dir: Utf8TempDir,
    access: Option<AccessPolicy>,
    upstream: Client,
    root: Utf8PathBuf,
    invocation: InvocationContext,
}

impl Fixture {
    /// A tool configured from `partial`, with the given name and parameters.
    fn new(name: &str, partial: Value, parameters: Value) -> Self {
        let partial: PartialToolConfig = serde_json::from_value(partial).unwrap();
        let mut app = AppConfig::new_test();
        app.conversation.tools.insert(
            name.to_owned(),
            ToolConfig::from_partial(partial, vec![]).unwrap(),
        );
        Self {
            definition: ToolDefinition {
                name: name.to_owned(),
                docs: ToolDocs::default(),
                parameters,
            },
            config: app.conversation.tools.get(name).unwrap(),
            builtins: builtin::BuiltinExecutors::new(),
            runner: no_commands(),
            command_plugins: CommandPlugins::new(PluginInit {
                workspace_id: "ws-abc".to_owned(),
                storage: Some("/tmp/.jp".into()),
                paths: PathsInfo::default(),
                config: json!({"user": {"name": "tester"}}),
                log_level: 0,
            }),
            dir: tempdir().unwrap(),
            access: None,
            upstream: Client::new(IndexMap::new()),
            root: "/tmp".into(),
            invocation: InvocationContext::default(),
        }
    }

    fn with_builtin(mut self, name: &str, tool: impl BuiltinTool + 'static) -> Self {
        self.builtins = self.builtins.register(name, tool);
        self
    }

    /// Put a `jp-{name}` binary on disk and admit it, with `options` as its
    /// plugin options.
    fn with_plugin(mut self, name: &str, options: Value) -> Self {
        let binary = self.binary(name);
        fs::write(&binary, "v1").unwrap();
        let Value::Object(options) = options else {
            panic!("plugin options are an object");
        };
        self.command_plugins = mem::take(&mut self.command_plugins).with(name, AdmittedPlugin {
            sha256: sha256_file(&binary).unwrap(),
            binary,
            options,
        });
        self
    }

    fn without_storage(mut self) -> Self {
        self.command_plugins = CommandPlugins::new(PluginInit::default());
        self
    }

    fn binary(&self, name: &str) -> Utf8PathBuf {
        self.dir.path().join(format!("jp-{name}"))
    }

    fn with_access(mut self, access: AccessPolicy) -> Self {
        self.access = Some(access);
        self
    }

    fn with_invocation(mut self, invocation: InvocationContext) -> Self {
        self.invocation = invocation;
        self
    }

    fn with_runner(mut self, runner: Arc<dyn ProcessRunner>) -> Self {
        self.runner = runner;
        self
    }

    fn execution(&self, id: &str, arguments: Value) -> Execution<'_> {
        Execution {
            definition: &self.definition,
            id: id.to_owned(),
            arguments,
            action: Action::Run,
            config: &self.config,
            root: &self.root,
            access: self.access.as_ref(),
            invocation: &self.invocation,
            builtins: &self.builtins,
            runner: &self.runner,
            command_plugins: &self.command_plugins,
            upstream: &self.upstream,
            cancellation: CancellationToken::new(),
            stderr: None,
        }
    }
}

#[test]
fn command_error_keeps_details_and_its_conversation_projection() {
    let output = br#"{"type":"error","message":"busy","trace":["upstream"],"transient":true}"#;
    let result = parse_command_output(output, b"", false).into_tool_result("test");
    assert_eq!(
        result.status,
        ToolStatus::Error(ErrorDetails {
            transient: true,
            trace: vec!["upstream".into()],
        })
    );
    assert!(result.is_error());
    assert_eq!(
        result.to_text(),
        r#"{"message":"busy","trace":["upstream"]}"#
    );
}

#[test]
fn test_execution_outcome_id() {
    let completed = ExecutionOutcome::Completed {
        id: "id1".to_string(),
        result: ToolResult::text(""),
    };
    assert_eq!(completed.id(), "id1");

    let needs_input = ExecutionOutcome::NeedsInput {
        id: "id2".to_string(),
        question: Question::text("q", "?").unwrap(),
    };
    assert_eq!(needs_input.id(), "id2");

    let cancelled = ExecutionOutcome::Cancelled {
        id: "id3".to_string(),
    };
    assert_eq!(cancelled.id(), "id3");
}

#[test]
fn test_execution_outcome_helper_methods() {
    let success = ExecutionOutcome::Completed {
        id: "1".to_string(),
        result: ToolResult::text("output"),
    };
    assert!(success.is_success());
    assert!(!success.needs_input());
    assert!(!success.is_cancelled());

    let failure = ExecutionOutcome::Completed {
        id: "2".to_string(),
        result: ToolResult::error("error"),
    };
    assert!(!failure.is_success());
    assert!(!failure.needs_input());
    assert!(!failure.is_cancelled());

    let needs_input = ExecutionOutcome::NeedsInput {
        id: "3".to_string(),
        question: Question::boolean("q", "?").unwrap(),
    };
    assert!(!needs_input.is_success());
    assert!(needs_input.needs_input());
    assert!(!needs_input.is_cancelled());

    let cancelled = ExecutionOutcome::Cancelled {
        id: "4".to_string(),
    };
    assert!(!cancelled.is_success());
    assert!(!cancelled.needs_input());
    assert!(cancelled.is_cancelled());
}

#[test]
fn parse_command_output_valid_needs_input() {
    let stdout = br#"{"type":"needs_input","question":{"id":"confirm","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    assert!(matches!(
        parse_command_output(stdout, b"", true),
        CommandResult::NeedsInput(_)
    ));
}

#[test]
fn parse_command_output_dotted_question_id_is_invalid_inquiry() {
    let stdout = br#"{"type":"needs_input","question":{"id":"a.b","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(matches!(
        result,
        CommandResult::InvalidInquiry { ref question_id } if question_id == "a.b"
    ));
    // Renders as a tool-level error, not raw text.
    assert!(result.into_tool_result("t").is_error());
}

#[test]
fn parse_command_output_empty_question_id_is_invalid_inquiry() {
    let stdout = br#"{"type":"needs_input","question":{"id":"","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(matches!(
        result,
        CommandResult::InvalidInquiry { ref question_id } if question_id.is_empty()
    ));
    assert!(result.into_tool_result("t").is_error());
}

#[test]
fn parse_command_output_legacy_answer_type_shape_is_malformed_inquiry() {
    // A stale local-tool binary emits the pre-082 externally-tagged answer
    // type (`"answer_type":"Boolean"`) instead of the internally-tagged
    // `{"type":"boolean"}` this build parses. The question id is valid, so
    // the payload must surface as a tool-level error rather than being handed
    // to the model as raw JSON.
    let stdout = br#"{"type":"needs_input","question":{"id":"apply_changes","text":"Apply?","answer_type":"Boolean","default":true}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(
        matches!(result, CommandResult::MalformedInquiry { .. }),
        "expected MalformedInquiry, got {result:?}"
    );
    // Renders as a tool-level error, not raw text.
    assert!(result.into_tool_result("fs_modify_file").is_error());
}

#[test]
fn parse_command_output_needs_input_missing_field_is_malformed_inquiry() {
    // A `needs_input` missing a required question field fails to deserialize;
    // with a valid id it is a malformed inquiry, not raw output.
    let stdout = br#"{"type":"needs_input","question":{"id":"confirm"}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(
        matches!(result, CommandResult::MalformedInquiry { .. }),
        "expected MalformedInquiry, got {result:?}"
    );
    assert!(result.into_tool_result("t").is_error());
}

#[test]
fn parse_command_output_non_outcome_is_raw() {
    assert!(matches!(
        parse_command_output(b"plain text", b"", true),
        CommandResult::RawOutput { .. }
    ));
}

#[test]
fn parse_command_output_non_needs_input_json_is_raw() {
    // Valid JSON that is not an `Outcome` and not a `needs_input` payload
    // stays raw output — the malformed-inquiry path must not swallow it.
    let stdout = br#"{"some":"object","the_tool":"did not use the protocol"}"#;
    assert!(matches!(
        parse_command_output(stdout, b"", true),
        CommandResult::RawOutput { .. }
    ));
}

/// Build a parameters schema from `(name, node, required)` triples.
fn schema<const N: usize>(properties: [(&str, Value, bool); N]) -> Value {
    let required = properties
        .iter()
        .filter(|(_, _, required)| *required)
        .map(|(name, _, _)| Value::String((*name).to_owned()))
        .collect::<Vec<_>>();
    let properties = properties
        .into_iter()
        .map(|(name, node, _)| (name.to_owned(), node))
        .collect::<Map<_, _>>();

    json!({ "type": "object", "properties": properties, "required": required })
}

/// A schema node of the given type.
fn param(kind: &str) -> Value {
    json!({ "type": kind })
}

#[tokio::test]
async fn local_tool_rejects_scalar_enum_on_array_parameter() {
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "parameters": {
            "tags": {
                "type": "array",
                "enum": ["projects/jp", "task", "idea"],
                "items": { "type": "string" }
            }
        }
    }))
    .unwrap();
    let tool = ToolConfig::from_partial(partial, vec![]).unwrap();
    let mut app = AppConfig::new_test();
    app.conversation
        .tools
        .insert("bear_note_create".to_owned(), tool);
    let config = app.conversation.tools.get("bear_note_create").unwrap();

    let error = resolve_tool("bear_note_create", &config, &Client::new(IndexMap::new()))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid schema at `conversation.tools.bear_note_create.parameters.tags.enum`: enum value \
         \"projects/jp\" has type string, but the schema requires array; use \
         `conversation.tools.bear_note_create.parameters.tags.items.enum` to constrain array \
         elements"
    );
}

#[tokio::test]
async fn execute_coerces_json_strings_before_calling_tool() {
    let fixture = Fixture::new(
        "echo_arguments",
        json!({"source": "builtin"}),
        schema([("start_line", param("integer"), false)]),
    )
    .with_builtin("echo_arguments", EchoArguments);

    let outcome = execute(
        &fixture.execution("call_1", json!({"start_line": "1"})),
        &Answers::new(),
    )
    .await
    .unwrap();

    let ExecutionOutcome::Completed { id, result, .. } = outcome else {
        panic!("expected completed tool call");
    };
    assert_eq!(id, "call_1");
    assert_eq!(result, ToolResult::text(r#"{"start_line":1}"#));
}

/// Run `command` under `ctx` and return the process it started, as the runner
/// was asked to start it.
async fn spawned(command: CommandConfig, ctx: Value) -> ProcessSpec {
    let runner = echoing();
    let dyn_runner: Arc<dyn ProcessRunner> = runner.clone();
    run_tool_command(
        &dyn_runner,
        command,
        ctx,
        "/tmp".into(),
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();

    let mut calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    calls.remove(0)
}

fn command(program: &str, args: &[&str]) -> CommandConfig {
    CommandConfig {
        program: program.to_owned(),
        args: args.iter().map(ToString::to_string).collect(),
        shell: false,
    }
}

/// Regression: `{{tool}}` must render as valid JSON, including `null` for null
/// fields (not Jinja2's `none`).
/// Originally fixed with `AutoEscape::Json`, now handled by the custom
/// formatter which JSON-serializes composite values while leaving scalars
/// alone.
#[tokio::test]
async fn test_run_tool_command_renders_null_args_as_valid_json() {
    let ctx = json!({
        "tool": {
            "name": "cargo_test",
            "arguments": {
                "package": "jp_workspace",
                "backtrace": null,
                "testname": null,
            },
            "answers": {},
            "options": {},
        },
        "context": {
            "action": "run",
            "root": "/tmp",
        },
    });

    let spec = spawned(command("echo", &["{{tool}}"]), ctx).await;

    // The rendered argument must be valid JSON with proper `null` values.
    let parsed: Value = serde_json::from_str(&spec.args[0]).unwrap_or_else(|e| {
        panic!(
            "run_tool_command rendered invalid JSON: {e}\n\nArgument: {}",
            spec.args[0]
        )
    });

    assert_eq!(parsed["arguments"]["package"], "jp_workspace");
    assert_eq!(parsed["arguments"]["backtrace"], Value::Null);
    assert_eq!(parsed["name"], "cargo_test");
}

/// Regression: scalar string interpolation must not be JSON-quoted.
/// A prior fix for the null-rendering bug set `AutoEscape::Json` globally,
/// which wrapped every string value in literal `"..."`, breaking templates like
/// `just rfd-draft {{tool.arguments.title}}` where tool authors expect the bare
/// value.
#[tokio::test]
async fn test_run_tool_command_renders_scalar_strings_raw() {
    let ctx = json!({
        "tool": {
            "arguments": { "title": "Hello World" },
        },
    });

    let spec = spawned(command("echo", &["{{tool.arguments.title}}"]), ctx).await;

    assert_eq!(spec.args, ["Hello World"]);
}

/// Null scalars render as literal `null` (not Jinja2's `none`, and not an empty
/// string).
/// This keeps the behavior consistent with how null appears inside
/// JSON-serialized composites.
#[tokio::test]
async fn test_run_tool_command_renders_null_scalar_as_literal_null() {
    let ctx = json!({
        "tool": { "arguments": { "maybe": null } },
    });

    let spec = spawned(command("echo", &["{{tool.arguments.maybe}}"]), ctx).await;

    assert_eq!(spec.args, ["null"]);
}

/// End-to-end sanity check for the rfd-draft regression: with the old
/// `AutoEscape::Json` behavior, `{{tool.arguments.title}}` rendered as
/// `"Assistant-Initiated ..."` (literal quotes), which then broke the
/// downstream `sed` command inside the just recipe.
/// Verify the title now reaches the subprocess as a clean argument.
#[tokio::test]
async fn test_run_tool_command_rfd_draft_title_rendering() {
    let ctx = json!({
        "tool": {
            "arguments": {
                "variant": "design",
                "title": "Assistant-Initiated User Inquiries via an ask_user Builtin",
            },
        },
    });

    // Mimic the real `just rfd-draft {{variant}} {{title}}` template.
    let spec = spawned(
        command("just", &[
            "rfd-draft",
            "{{tool.arguments.variant}}",
            "{{tool.arguments.title}}",
        ]),
        ctx,
    )
    .await;

    assert_eq!(spec.program, "just");
    assert_eq!(spec.args, [
        "rfd-draft",
        "design",
        "Assistant-Initiated User Inquiries via an ask_user Builtin",
    ]);
}

/// The `tojson` filter still works for tool authors who want explicit
/// JSON-quoted strings (e.g. when hand-crafting a JSON literal).
/// Safe strings produced by `tojson` must pass through the custom formatter
/// unchanged — no double-encoding.
#[tokio::test]
async fn test_run_tool_command_tojson_filter_on_scalar_still_works() {
    let ctx = json!({
        "tool": { "arguments": { "title": "Hello" } },
    });

    let spec = spawned(command("echo", &["{{tool.arguments.title | tojson}}"]), ctx).await;

    assert_eq!(spec.args, ["\"Hello\""]);
}

/// A shell-mode command runs as a script: the program is shell syntax used
/// verbatim, and each argument is quoted so a multi-word one stays one word.
#[tokio::test]
async fn test_run_tool_command_runs_a_shell_command_as_a_script() {
    let ctx = json!({
        "tool": { "arguments": { "title": "two words" } },
    });

    let spec = spawned(
        CommandConfig {
            shell: true,
            ..command("grep -c", &["{{tool.arguments.title}}", "notes.md"])
        },
        ctx,
    )
    .await;

    assert_eq!(spec.program, "sh");
    assert_eq!(spec.args, ["-c", "grep -c 'two words' notes.md"]);
    assert_eq!(spec.dir, "/tmp");
}

/// A Ctrl-C at the terminal does not reach a tool command, which JP stops
/// through its cancellation token once the user has said what the interrupt
/// means.
#[tokio::test]
async fn test_run_tool_command_keeps_a_terminal_ctrl_c_from_the_tool() {
    let spec = spawned(command("cargo", &["test"]), json!({})).await;

    assert!(spec.own_process_group);
}

/// Regression: the `run` path must surface the invocation's workspace and
/// conversation IDs to the tool command via `context.workspace_id` and
/// `context.conversation_id`.
/// A non-empty `InvocationContext` pins the wiring so the fields can't be
/// silently dropped or emptied.
#[tokio::test]
async fn test_execute_local_exposes_invocation_ids_in_context() {
    let fixture = Fixture::new(
        "echo_ids",
        json!({
            "source": "local",
            "command": "echo {{context.workspace_id}}-{{context.conversation_id}}",
        }),
        schema([]),
    )
    .with_invocation(InvocationContext {
        workspace_id: "ws-abc".to_owned(),
        conversation_id: "conv-xyz".to_owned(),
    })
    .with_runner(echoing());

    let outcome = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .expect("execution succeeds");

    match outcome {
        ExecutionOutcome::Completed { result, .. } => {
            assert_eq!(result, ToolResult::text("ws-abc-conv-xyz\n"));
        }
        other => panic!("expected completed success, got: {other:?}"),
    }
}

/// A built-in that reports it ran, so dispatch can be observed.
struct ReachedBuiltin;

#[async_trait::async_trait]
impl builtin::BuiltinTool for ReachedBuiltin {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> jp_tool::Outcome {
        "reached".into()
    }
}

/// A built-in tool may be keyed differently from the implementation it names:
/// `source = "builtin.describe_tools"` under a `docs` key.
/// Dispatch keys on the source's tool name, matching how the local and MCP
/// paths treat theirs.
#[tokio::test]
async fn test_execute_builtin_dispatches_on_source_name() {
    let fixture = Fixture::new(
        "docs",
        json!({"source": "builtin.describe_tools"}),
        schema([]),
    )
    .with_builtin("describe_tools", ReachedBuiltin);

    let outcome = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .expect("execution succeeds");

    match outcome {
        ExecutionOutcome::Completed { result, .. } => {
            assert_eq!(result, ToolResult::text("reached"));
        }
        other => panic!("expected completed success, got: {other:?}"),
    }
}

/// Regression for RFD 081: `tool_definitions` keeps a *forced* tool that is
/// merely disabled (`OFF`), but always drops a locked-off tool (`state =
/// false`, `allow_toggle = never`) even when it is forced.
#[tokio::test]
async fn test_tool_definitions_forced_tool_drops_locked_off() {
    use jp_config::{
        AppConfig, Config,
        conversation::tool::{PartialToolConfig, ToolConfig},
    };

    let off: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo off",
        "enable": false,
    }))
    .expect("valid partial tool config");
    let locked_off: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo locked",
        "enable": { "state": false, "allow_toggle": "never" },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "off_tool".to_owned(),
        ToolConfig::from_partial(off, vec![]).expect("resolved tool config"),
    );
    cfg.conversation.tools.insert(
        "locked_off_tool".to_owned(),
        ToolConfig::from_partial(locked_off, vec![]).expect("resolved tool config"),
    );

    let mcp_client = Client::new(IndexMap::new());

    // Forcing the toggleable OFF tool keeps it in the definitions.
    let defs = tool_definitions(cfg.conversation.tools.iter(), &mcp_client, Some("off_tool"))
        .await
        .expect("tool definitions resolve");
    assert!(
        defs.iter().any(|d| d.name == "off_tool"),
        "a forced toggleable OFF tool must be kept"
    );

    // Forcing the locked-off tool still drops it.
    let defs = tool_definitions(
        cfg.conversation.tools.iter(),
        &mcp_client,
        Some("locked_off_tool"),
    )
    .await
    .expect("tool definitions resolve");
    assert!(
        !defs.iter().any(|d| d.name == "locked_off_tool"),
        "a locked-off tool must be dropped even when forced"
    );
}

/// A tool whose schema cannot be resolved is dropped from the request rather
/// than failing the whole query, mirroring how an unavailable MCP server is
/// handled.
#[tokio::test]
async fn tool_with_an_unresolvable_schema_is_skipped() {
    let broken: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo broken",
        "parameters": { "tags": { "type": "array" } },
    }))
    .expect("valid partial tool config");
    let healthy: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo fine",
        "parameters": { "path": { "type": "string" } },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "broken_tool".to_owned(),
        ToolConfig::from_partial(broken, vec![]).expect("resolved tool config"),
    );
    cfg.conversation.tools.insert(
        "healthy_tool".to_owned(),
        ToolConfig::from_partial(healthy, vec![]).expect("resolved tool config"),
    );

    let defs = tool_definitions(
        cfg.conversation.tools.iter(),
        &Client::new(IndexMap::new()),
        None,
    )
    .await
    .expect("a broken tool must not fail the query");

    let names = defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, vec!["healthy_tool"]);
}

/// A plugin that prints `lines`, each serialized as one protocol line, and
/// exits successfully.
fn plugin_printing(lines: &[Value]) -> Arc<MockProcessRunner> {
    let stdout: String = lines.iter().map(|line| line.to_string() + "\n").collect();
    Arc::new(MockProcessRunner::responding(move |_| {
        Ok(ProcessOutput {
            stdout: stdout.clone(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }))
}

/// A plugin that answers with `outcome` and exits.
fn plugin_answering(outcome: Value) -> Arc<MockProcessRunner> {
    plugin_printing(&[
        json!({"type": "ready", "protocol": 10}),
        Value::Object(Map::from_iter([
            ("type".to_owned(), json!("tool_outcome")),
            ("outcome".to_owned(), outcome),
        ])),
        json!({"type": "exit", "code": 0}),
    ])
}

/// The `init` the plugin was started with, read back from its stdin.
fn init_sent(runner: &MockProcessRunner) -> Value {
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "the plugin ran once: {calls:?}");
    let stdin = calls[0].stdin.as_deref().expect("init on stdin");
    assert!(stdin.ends_with('\n'), "one line: {stdin:?}");
    serde_json::from_str(stdin).unwrap()
}

#[tokio::test]
async fn command_tool_runs_its_plugin_with_the_call_on_stdin() {
    let runner = plugin_answering(json!({"type": "success", "content": "Created T-0abc123"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create", "options": {"mode": "strict"}}),
        schema([
            ("title", param("string"), true),
            ("kind", json!({"type": "string", "default": "bug"}), false),
        ]),
    )
    .with_plugin("ticket", json!({"dir": "packages/foo/tickets"}))
    .with_runner(runner.clone())
    .with_invocation(InvocationContext {
        workspace_id: "ws-abc".to_owned(),
        conversation_id: "jp-c17000000000".to_owned(),
    });

    let answers = Answers::from([("confirm".to_owned(), json!(true))]);
    let outcome = execute(
        &fixture.execution("call-1", json!({"title": "Fix it"})),
        &answers,
    )
    .await
    .unwrap();

    let ExecutionOutcome::Completed { id, result } = outcome else {
        panic!("expected a completed call, got {outcome:?}");
    };
    assert_eq!(id, "call-1");
    assert_eq!(result, ToolResult::text("Created T-0abc123"));

    let spec = &runner.calls()[0];
    assert_eq!(spec.program, fixture.binary("ticket").as_str());
    assert_eq!(spec.args, Vec::<String>::new());
    assert_eq!(spec.dir, Utf8PathBuf::from("/tmp"));
    assert!(spec.own_process_group, "a Ctrl-C must not reach the plugin");

    assert_eq!(
        init_sent(&runner),
        json!({
            "type": "init",
            "version": PROTOCOL_VERSION,
            "workspace": {"root": "/tmp", "storage": "/tmp/.jp", "id": "ws-abc"},
            "paths": {},
            "config": {"user": {"name": "tester"}},
            "options": {"dir": "packages/foo/tickets"},
            "args": [],
            "log_level": 0,
            "output_format": "text",
            "tool": {
                "action": "run",
                "name": "create",
                // The configured default is applied before the plugin sees the
                // call, as it is for a local tool.
                "arguments": {"title": "Fix it", "kind": "bug"},
                "answers": {"confirm": true},
                "options": {"mode": "strict"},
                "conversation": "jp-c17000000000"
            }
        })
    );
}

/// A tool that formats its own arguments is asked to, through the same `init`,
/// with the action saying it is a description and not a run.
#[tokio::test]
async fn command_tool_formatting_its_arguments_says_so_in_init() {
    let runner = plugin_answering(json!({"type": "success", "content": "File bug: Fix it"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create", "style": {"parameters": "tool"}}),
        schema([("title", param("string"), true)]),
    )
    .with_plugin("ticket", json!({"dir": "packages/foo/tickets"}))
    .with_runner(runner.clone());

    let mut execution = fixture.execution("call-1", json!({"title": "Fix it"}));
    execution.action = Action::FormatArguments;
    let outcome = execute(&execution, &Answers::new()).await.unwrap();

    let ExecutionOutcome::Completed { result, .. } = outcome else {
        panic!("expected a completed call, got {outcome:?}");
    };
    assert_eq!(result, ToolResult::text("File bug: Fix it"));
    let init = init_sent(&runner);
    assert_eq!(init["tool"]["action"], "format_arguments");
    assert_eq!(init["options"], json!({"dir": "packages/foo/tickets"}));
}

#[tokio::test]
async fn command_tool_run_says_so_in_init() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone());

    execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    assert_eq!(init_sent(&runner)["tool"]["action"], "run");
}

/// A local tool styled `parameters = "tool"` formats its own arguments by
/// running its own command, with the action in `context.action`.
#[tokio::test]
async fn local_tool_formatting_its_arguments_runs_its_command_for_that_action() {
    let fixture = Fixture::new(
        "word_count",
        json!({
            "source": "local",
            "command": {"program": "word_count", "args": ["{{context.action}}"], "shell": false},
            "style": {"parameters": "tool"},
        }),
        schema([]),
    )
    .with_runner(echoing());

    let mut execution = fixture.execution("call-1", json!({}));
    execution.action = Action::FormatArguments;
    let outcome = execute(&execution, &Answers::new()).await.unwrap();

    let ExecutionOutcome::Completed { result, .. } = outcome else {
        panic!("expected a completed call, got {outcome:?}");
    };
    assert_eq!(result, ToolResult::text("format_arguments\n"));
}

#[tokio::test]
async fn command_tool_without_a_tool_name_uses_the_configured_key() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new("labels", json!({"source": "command.ticket"}), schema([]))
        .with_plugin("ticket", json!({}))
        .with_runner(runner.clone());

    execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    assert_eq!(init_sent(&runner)["tool"]["name"], "labels");
}

/// The compiled policy reaches the plugin, which enforces it, as it reaches a
/// local tool's command through `context.access`.
#[tokio::test]
async fn command_tool_receives_the_compiled_access_policy() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone())
    .with_access(AccessPolicy {
        env: vec![EnvRule {
            name: "AWS_*".to_owned(),
            read: false,
        }],
        ..AccessPolicy::default()
    });

    execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    assert_eq!(
        init_sent(&runner)["tool"]["access"],
        json!({"fs": [], "net": [], "env": [{"name": "AWS_*", "read": false}]})
    );
}

#[tokio::test]
async fn command_tool_with_invalid_arguments_never_reaches_the_plugin() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([("title", param("string"), true)]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone());

    let outcome = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    let ExecutionOutcome::Completed { result, .. } = outcome else {
        panic!("expected a completed call, got {outcome:?}");
    };
    assert!(result.is_error());
    assert!(
        result.to_text().starts_with("Invalid arguments: "),
        "got: {}",
        result.to_text()
    );
    assert_eq!(runner.calls(), vec![]);
}

#[tokio::test]
async fn command_tool_question_ends_the_attempt() {
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(plugin_answering(json!({
        "type": "needs_input",
        "question": {"id": "confirm", "text": "File it?", "answer_type": {"type": "boolean"}}
    })));

    let outcome = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    let ExecutionOutcome::NeedsInput { id, question } = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    assert_eq!(id, "call-1");
    assert_eq!(question.id.as_str(), "confirm");
}

#[tokio::test]
async fn command_tool_error_outcome_keeps_its_details() {
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(plugin_answering(json!({
        "type": "error", "message": "busy", "trace": ["lock held"], "transient": true
    })));

    let outcome = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap();

    let ExecutionOutcome::Completed { result, .. } = outcome else {
        panic!("expected a completed call, got {outcome:?}");
    };
    assert_eq!(
        result.status,
        ToolStatus::Error(ErrorDetails {
            transient: true,
            trace: vec!["lock held".into()],
        })
    );
}

/// A plugin that sent `tool_outcome` said it was answering in the outcome
/// shape, so anything else is a protocol fault, not text for the model.
#[tokio::test]
async fn command_tool_outcome_in_the_wrong_shape_is_malformed_output() {
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(plugin_answering(json!({"content": "no type tag"})));

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, ToolError::MalformedOutput(_)),
        "got: {error:?}"
    );
}

#[tokio::test]
async fn command_tool_failing_exit_reports_the_plugin_reason() {
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(plugin_printing(&[
        json!({"type": "ready", "protocol": 10}),
        json!({"type": "exit", "code": 1, "reason": "No ticket T-0abc123."}),
    ]));

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Command plugin `ticket` failed: No ticket T-0abc123."
    );
}

/// A plugin that sends a successful outcome and then crashes has not finished
/// the call; the model is told it failed, not that it succeeded.
#[tokio::test]
async fn command_tool_that_crashes_after_answering_fails() {
    let runner = Arc::new(MockProcessRunner::responding(|_| {
        Ok(ProcessOutput {
            stdout: format!(
                "{}\n",
                json!({"type": "tool_outcome", "outcome": {"type": "success", "content": "ok"}})
            ),
            stderr: "thread 'main' panicked".to_owned(),
            status: ExitCode::from(Some(101)),
        })
    }));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner);

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Command plugin `ticket` failed: answered the tool call, then exited without sending \
         `exit`"
    );
}

#[tokio::test]
async fn command_tool_whose_binary_cannot_start_fails() {
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}));
    let binary = fixture.binary("ticket");
    let fixture = fixture.with_runner(Arc::new(
        MockProcessRunner::builder().expect_any().fails_to_spawn(),
    ));

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("Command plugin `ticket` failed: failed to start {binary}: entity not found")
    );
}

/// Only plugins the host admitted for the turn run; any other is refused before
/// anything is spawned.
#[tokio::test]
async fn command_tool_of_a_plugin_not_admitted_never_runs() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_runner(runner.clone());

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Command plugin `ticket` cannot be run from here: it was not admitted for this turn"
    );
    assert_eq!(runner.calls(), vec![]);
}

/// Admission decided on a binary's contents.
/// One replaced during the turn is refused at its next call rather than run on
/// the strength of the old one.
#[tokio::test]
async fn command_tool_whose_binary_changed_after_admission_never_runs() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone());
    fs::write(fixture.binary("ticket"), "v2").unwrap();

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "Command plugin `ticket` cannot be run from here: {} changed since it was admitted at \
             the start of this turn",
            fixture.binary("ticket")
        )
    );
    assert_eq!(runner.calls(), vec![]);
}

#[tokio::test]
async fn command_tool_without_workspace_storage_never_runs() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .without_storage()
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone());

    let error = execute(&fixture.execution("call-1", json!({})), &Answers::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Command plugin `ticket` cannot be run from here: the workspace has no storage configured"
    );
    assert_eq!(runner.calls(), vec![]);
}

/// The cancellation token reaches the runner, which is what stops a running
/// plugin.
/// The mock reports a run whose token is cancelled as cancelled, so a token
/// that never reached it would come back as a completed call.
#[tokio::test]
async fn cancelling_a_command_tool_reaches_the_runner() {
    let runner = plugin_answering(json!({"type": "success", "content": "ok"}));
    let fixture = Fixture::new(
        "ticket_create",
        json!({"source": "command.ticket.create"}),
        schema([]),
    )
    .with_plugin("ticket", json!({}))
    .with_runner(runner.clone());

    let execution = fixture.execution("call-1", json!({}));
    execution.cancellation.cancel();

    let outcome = execute(&execution, &Answers::new()).await.unwrap();

    assert!(outcome.is_cancelled(), "got {outcome:?}");
    assert_eq!(runner.calls().len(), 1);
}

/// Naming a tool with `--tool` is an explicit request for it, so its schema
/// error surfaces instead of the tool silently disappearing.
#[tokio::test]
async fn forced_tool_with_an_unresolvable_schema_still_errors() {
    let broken: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo broken",
        "parameters": { "tags": { "type": "array" } },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "broken_tool".to_owned(),
        ToolConfig::from_partial(broken, vec![]).expect("resolved tool config"),
    );

    let error = tool_definitions(
        cfg.conversation.tools.iter(),
        &Client::new(IndexMap::new()),
        Some("broken_tool"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid schema at `conversation.tools.broken_tool.parameters.tags.items`: array schemas \
         must declare an item schema"
    );
}
