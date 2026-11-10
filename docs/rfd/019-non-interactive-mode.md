# RFD 019: Non-Interactive Mode

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01
- **Summary**: Proposes a `--non-interactive` flag, configurable detached prompt
  policies, and a four-channel output model for headless runs.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Split into two focused RFDs:
>
> - [RFD 048: Four-Channel Output Model][RFD 048]: stdout, stderr, `/dev/tty`,
>   and log file separation with `Printer` integration.
> - [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]:
>   `--non-interactive` flag, detached policies, `exclusive` property, and
>   prompt routing.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces a `--non-interactive` CLI flag, a configurable detached
prompt policy for handling inquiries when no interactive client is attached, and
an `exclusive` property that marks questions as human-only.
It also separates output into four channels: stdout for assistant output, stderr
for terminal chrome, `/dev/tty` for interactive prompts, and a log file for
tracing.

## Motivation

When running without a TTY, JP falls back to hardcoded defaults:

- Permission prompts (`RunMode::Ask|Edit`) are auto-approved when `!is_tty`.
- User-targeted tool questions are rerouted to the LLM inquiry backend.
- Result delivery prompts are auto-delivered.

These defaults cover simple cases, but users cannot:

- Opt into non-interactive mode from an interactive terminal (for example, in
  scripts).
- Configure separate fallback strategies for different prompt types.
- Mark questions as human-only to prevent the LLM from answering them
  automatically.
- Pipe JP's output cleanly, because progress indicators and tool call headers
  share stdout with the assistant response (breaking pipelines such as `echo
  "fix it" | jp query | jq`).

## Design

### Detached Prompt Policy

When an inquiry ([RFD 018]) arrives and no interactive client is attached, JP
applies a **detached prompt policy** configured per inquiry kind:

| Mode       | Behavior                                                 |
| ---------- | -------------------------------------------------------- |
| `auto`     | Auto-approve (`RunTool`/`DeliverToolResult`) or route to |
|            | LLM inquiry (`ToolQuestion`). Fail if `exclusive=true`.  |
| `defaults` | Use the question's `default` value, or fail if none.     |
| `deny`     | Fail the tool call with a descriptive error.             |

The default mode is **`deny`**: nothing runs unattended unless the user
explicitly opts in.
A non-interactive run that hits an unresolved prompt fails with a descriptive
error instead of silently auto-approving.

### The `exclusive` Property

The `exclusive` property marks a question as human-only for prompts the LLM
cannot meaningfully answer, such as:

- "Enter your SSH passphrase"
- "Confirm deletion of production database"
- "Choose which local git identity to use"

When the detached policy is `auto`, exclusive questions fail instead of routing
to the LLM.

At the type level, `Inquiry::exclusive()` ([RFD 018]) marks `RunTool` and
`DeliverToolResult` inquiries as inherently exclusive, while `ToolQuestion`
inquiries are non-exclusive by default.

Tool authors set the default on `Question`:

```rust
Question {
    id: "confirm_force_push".into(),
    text: "Force push to remote?".into(),
    answer_type: AnswerType::Boolean,
    default: None,
    exclusive: true,
}
```

Users can override `exclusive` per question in configuration, including setting
`exclusive = false` on questions a tool author marked as exclusive:

```toml
[conversation.tools.git.questions.confirm_force_push]
target = "user"
exclusive = true
```

### Prompt Routing

This RFD extends `route_prompt` from [RFD 018] with the detached policy and
optional `exclusive` override:

```rust
fn route_prompt(
    inquiry: &Inquiry,
    has_client: bool,
    policy: DetachedMode,
    config_exclusive: Option<bool>,
) -> PromptAction {
    if has_client {
        return PromptAction::PromptClient;
    }

    let exclusive = config_exclusive.unwrap_or_else(|| inquiry.exclusive());

    match policy {
        DetachedMode::Auto if exclusive => PromptAction::Fail,
        DetachedMode::Auto => match inquiry {
            Inquiry::RunTool { .. } => PromptAction::AutoApprove,
            Inquiry::DeliverToolResult { .. } => PromptAction::AutoDeliver,
            Inquiry::ToolQuestion { .. } => PromptAction::LlmInquiry,
        },
        DetachedMode::Defaults => PromptAction::UseDefault,
        DetachedMode::Deny => PromptAction::Fail,
    }
}
```

Scattered `is_tty` checks in the coordinator collapse into `route_prompt` calls,
with each call site passing the inquiry and any configured `exclusive` override.

#### Standard Input

`stdin` is reserved for query content and context injection (such as `cat
file.rs | jp query "fix this"`) and is **never** used to answer prompts.
Prompt answers come only from `/dev/tty` in interactive mode or from the
detached policy in non-interactive mode.

For example, `echo "y" | jp query "do the thing"` treats `"y"` as query content
rather than an answer to a tool permission prompt.
If the query hits a prompt, JP prompts the user on `/dev/tty` when available, or
applies the detached policy otherwise.

#### Determining `has_client`

`has_client` is `true` when an interactive user can answer prompts:

1. If `--non-interactive` is passed, `has_client` is `false`.
2. Otherwise, JP attempts to open `/dev/tty`.
   If it succeeds, `has_client` is `true`.
3. If `/dev/tty` cannot be opened (for example, in cron, systemd, SSH without
   `-t`, or a daemonized process with no controlling terminal), `has_client` is
   `false`.

Replacing the existing `stdout.is_terminal()` heuristic with `/dev/tty`
availability decouples interactivity from stdout redirection.
In `jp query | less`, stdout is connected to a pipe, but `/dev/tty` remains
available so the user can still answer prompts at the terminal.
Conversely, in `echo foo | jp query`, if `/dev/tty` is unavailable, `has_client`
is `false` even when stdout is a TTY.

### Configuration

#### Scalar Shorthand

A scalar string sets one policy for all inquiry kinds (`run`, `deliver`, and
`tool`), following the same scalar-or-struct convention as `command` (which
accepts a string or `{ program, args, shell }`):

```toml
[conversation.tools.defaults]
detached = "deny"
```

Setting `detached = "auto"` is shorthand for `detached = { run = "auto", deliver
= "auto", tool = "auto" }`.

#### Per Inquiry Kind

A table configures each inquiry kind independently:

```toml
[conversation.tools.defaults.detached]
run = "auto"
deliver = "auto"
tool = "deny"
```

#### Per Tool

Individual tools can override the defaults using either the scalar or table
form:

```toml
[conversation.tools.fs_modify_file]
detached = "deny"

[conversation.tools.cargo_check.detached]
run = "auto"
tool = "auto"
```

#### Resolution Order

JP resolves the policy for a tool and inquiry kind by taking the first match in
this cascade, following the same merge pattern `ToolConfigWithDefaults` uses for
`run`/`result`:

```
1. tools.<name>.detached.<kind>     (per-tool, per-kind)
2. tools.<name>.detached            (per-tool scalar)
3. tools.defaults.detached.<kind>   (global per-kind)
4. tools.defaults.detached          (global scalar)
5. "deny"                           (hardcoded safe default)
```

### Relationship to Existing Config

Existing config fields remain the **attached** (interactive) policies, and the
`detached` config applies only when no client is attached, introducing no
breaking changes to existing configs:

| Inquiry kind        | Attached policy          | Detached policy               |
| ------------------- | ------------------------ | ----------------------------- |
| `RunTool`           | `run` (ask/unattended/…) | `detached.run`                |
| `DeliverToolResult` | `result` (unattended/…)  | `detached.deliver`            |
| `ToolQuestion`      | `questions.<id>.target`  | `detached.tool` + `exclusive` |

### CLI Flag

```
jp query --non-interactive "Fix the bug"
```

`--non-interactive` forces detached prompt routing even when a TTY is present,
so scripts run from an interactive terminal never block on prompts.
When no controlling TTY is detected, JP behaves as if `--non-interactive` were
passed.

### Output Channel Separation

Instead of writing tracing to stderr and all other output to stdout, JP
separates output into four single-purpose channels:

| Channel        | Purpose                                |
| -------------- | -------------------------------------- |
| **stdout**     | Assistant responses, structured data   |
| **stderr**     | Chrome: progress, tool headers, status |
| **`/dev/tty`** | Interactive prompts (see below)        |
| **Log file**   | Tracing logs (`-v`)                    |

**stdout** contains only assistant output (or the structured JSON response when
`--format json` is used), so `jp query | jq`, `jp query > answer.txt`, and `jp
query | less` work without special-casing whether stdout is a TTY.

**stderr** contains terminal chrome: progress indicators, tool call headers, and
status messages.
In an unredirected terminal session, stdout and stderr both display on the same
screen, preserving the interleaved experience while keeping redirected streams
separate.

**Tracing logs** (`-v` through `-vvvvv`) are written to a log file rather than
stderr, preventing tracing output from mixing with chrome when stderr is
redirected (for example, `jp query 2> chrome.log` captures chrome only).
`--log-file` or `JP_LOG_FILE` configures the log file path (defaulting to
`~/.local/share/jp/logs/`), `--log-format` selects the log format (text or
JSON), and `--log-file=-` writes tracing to stderr for users who want logs and
chrome on the same stream.

**Output formatting** (`--format`) applies to both stdout and stderr: `--format
json` renders both channels as NDJSON, and `--format auto` resolves to
ANSI-formatted `text-pretty` when stdout is a terminal or plain `text`
otherwise.
`stdout.is_terminal()` and `/dev/tty` availability remain independent checks
with distinct roles:

- `stdout.is_terminal()` determines whether the consumer can handle ANSI escape
  codes.
- `/dev/tty` availability determines whether a user can answer interactive
  prompts (`has_client`).

### Prompt I/O Channel

When available, the controlling terminal device (`/dev/tty`) handles both
rendering and input for inquiry prompts (tool permissions, tool questions, and
result delivery confirmations).
Following the same pattern as git (password prompts), fzf (interactive UI), and
sudo (password entry), `/dev/tty` bypasses stdio redirections: even `jp query >
out.txt 2> err.txt` still renders prompts on the terminal, and `jp query | less`
sends assistant output to `less` via stdout, chrome to stderr, and tool prompts
to `/dev/tty` without the channels interfering.

When `/dev/tty` is unavailable, prompts cannot be rendered and `has_client` is
`false`, so the detached policy applies.

### Integration with `Printer`

`Printer` manages all output channels except tracing (which is written to the
log file by the `tracing` subscriber), preserving a single point of output for
testing and mocking.
`Printer` gains a `Tty` output target alongside `Out` and `Err`:

```rust
pub enum PrintTarget {
    Out,    // stdout — assistant output, structured data
    Err,    // stderr — chrome, progress, status
    Tty,    // /dev/tty — interactive prompts
}
```

The `Tty` target:

- Always renders with ANSI escape codes, since `/dev/tty` is a terminal by
  definition.
- Ignores `--format`, since prompts are interactive UI rather than data output.
- Opens `/dev/tty` lazily on the first call to `tty_writer()`, avoiding
  unnecessary opens for commands that never prompt.
- Returns an error when `/dev/tty` is unavailable, setting `has_client = false`.

Because `PromptBackend` already accepts a `writer` parameter,
`TerminalPromptBackend` wires to `printer.tty_writer()` instead of
`printer.out_writer()`, and `inquire` reads input from `/dev/tty` instead of
stdin.
In tests, the mock `Printer` provides an in-memory buffer for the `Tty` target
so prompt rendering can be asserted independently of stdout and stderr.

### Platform Portability

The Unix `/dev/tty` and `flock` APIs have direct Windows equivalents:

| Unix          | Windows                  | Rust crate               |
| ------------- | ------------------------ | ------------------------ |
| `/dev/tty`    | `CONIN$` / `CONOUT$`     | `crossterm`, `termwiz`   |
| `ttyname(fd)` | Console handle detection | `$JP_SESSION` as primary |

`Printer` and `PromptBackend` hide these platform-specific details:
`tty_writer()` opens `/dev/tty` on Unix and `CONIN$`/`CONOUT$` on Windows.

## Drawbacks

- **Larger config surface.** The detached policy adds a scalar-or-struct setting
  and a four-level resolution cascade, increasing documentation and mental
  overhead.
- **Breaking change in non-TTY behavior.** Defaulting to `deny` replaces the
  implicit non-TTY behavior (auto-approving permissions and rerouting questions
  to the LLM).
  Although replacing an unsafe default is intentional, users who rely on
  unattended piped runs must add `detached = "auto"` to their config.
- **Output separation changes redirected rendering.** Because chrome (progress,
  tool headers) goes to stderr and assistant output goes to stdout, users who
  redirect only stdout to a file capture only the assistant's final answer
  rather than the interleaved rendering shown in a terminal.

## Alternatives

### Single detached policy for all inquiry kinds

A single `detached = "auto"` setting could cover permissions, result delivery,
and tool questions together.
However, each inquiry kind has different risk characteristics: auto-approving a
permission prompt (after the LLM has already decided to call the tool) differs
from auto-answering a tool question (where the LLM may lack context).
Users need independent control over each kind.

### `exclusive` as a third `QuestionTarget` variant

Adding `QuestionTarget::UserOnly` instead of a boolean flag would conflate two
orthogonal properties: `target` specifies who answers a question in interactive
mode, whereas `exclusive` specifies whether that target can be overridden when
unavailable.
At the type level, `RunTool` and `DeliverToolResult` are inherently exclusive
via `Inquiry::exclusive()`, which a `QuestionTarget` variant cannot express.

### `auto` as the default detached policy

Defaulting to `auto` would preserve existing non-TTY behavior, but silently
auto-approves tool execution without user consent.
Defaulting to `deny` is safer because users must opt into automation explicitly.

### Environment variable instead of CLI flag

An environment variable such as `JP_FRONTEND=noninteractive` (modeled on
`DEBIAN_FRONTEND`) could control non-interactive mode instead of a CLI flag.
A CLI flag is more discoverable and consistent with JP's existing flag
conventions, though both can coexist with the environment variable as an alias.

## Non-Goals

- **Background execution and prompt queuing.** Running conversations as detached
  background processes, the `queue` detached policy, and attach IPC are future
  work that builds on the detached policy infrastructure established here.

  > [!TIP]
  > [RFD 027] introduces a client-server query architecture that makes detached
  > background execution and live re-attachment the default execution model,
  > building directly on the detached policies defined here.
  > The `queue` policy is renamed `defer`.

- **New inquiry variants.** This RFD uses the `Inquiry` enum from [RFD 018]
  as-is.

## Risks and Open Questions

### Interaction with the stateful tool protocol

[RFD 009] introduces stateful tools with `spawn`/`fetch`/`apply` actions.
The per-action permission model (prompt on `spawn`, auto-run `fetch`/`apply`)
maps to the detached policy, but the details need alignment during
implementation.

### Config cascade complexity

The four-level resolution cascade can be hard to debug when overrides interact
across global and per-tool settings.
A `jp config show --effective <tool>` command that displays the resolved
detached policy per inquiry kind would make the active configuration
inspectable.

### `exclusive` override direction

Allowing users to override `exclusive = true` (set by tool authors) to `false`
gives the user final control, but can lead to unsafe behavior for questions that
genuinely require human judgment.
Documentation must make these implications clear.

### `result` vs `deliver` naming

The `DeliverToolResult` inquiry's config key is `deliver`, whereas the existing
attached config field is `result`.
Implementation must resolve whether to use `deliver` everywhere (and alias
`result` via serde) or keep `result` as the config key to match the existing
field.

## Implementation Plan

### Phase 1: Detached Policy Config

Add the scalar-or-struct `detached` config field to `ToolsDefaultsConfig` and
`ToolConfig`, implement the `DetachedMode` enum (`auto`, `defaults`, `deny`) and
the config resolution cascade, and add the `exclusive` field to `Question` in
`jp_tool` and `QuestionConfig` in `jp_config`.
This phase parses the new config without consulting it at runtime, introducing
no behavioral changes, and can be merged independently.

### Phase 2: Routing Integration

Depends on [RFD 018] (the `Inquiry` enum) and Phase 1.
Replace `is_tty` checks in the coordinator with `route_prompt()` calls that
consult the resolved detached config, and add the `--non-interactive` CLI flag.

### Phase 3: Output Channel Separation

Add `PrintTarget::Tty` to `Printer`, wire `TerminalPromptBackend` to `/dev/tty`
via `Printer::tty_writer()`, route assistant output to stdout and chrome to
stderr unconditionally, and move tracing logs from stderr to a log file.
This phase is independent of Phases 1 and 2 and can be merged at any point.

## References

- [RFD 018: Typed Inquiry System][RFD 018]: Defines the `Inquiry` enum that this
  RFD's routing logic builds on.
- [RFD 009: Stateful Tool Protocol][RFD 009]: Defines the per-action permission
  model that interacts with the detached policy.
- `DEBIAN_FRONTEND=noninteractive`: Precedent for a non-interactive policy.
- ssh `BatchMode`: Precedent for a "fail on prompt" policy.
- `curl` / `git`: Precedent for stdout/stderr output separation.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
