use async_trait::async_trait;
use camino::Utf8PathBuf;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{CommandConfig, PartialToolConfig, ToolConfig, ToolConfigWithDefaults},
};
use jp_tool::{Outcome, ToolDefinition, ToolDocs};
use serde_json::Map;

use super::*;
use crate::{
    Client,
    server::testing::{echoing, no_commands},
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
            upstream: Client::new(IndexMap::new()),
            root: "/tmp".into(),
            invocation: InvocationContext::default(),
        }
    }

    fn with_builtin(mut self, name: &str, tool: impl BuiltinTool + 'static) -> Self {
        self.builtins = self.builtins.register(name, tool);
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
            access: None,
            invocation: &self.invocation,
            builtins: &self.builtins,
            runner: &self.runner,
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
