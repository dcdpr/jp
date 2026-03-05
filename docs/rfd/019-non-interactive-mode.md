# RFD 019: Non-Interactive Mode

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01

## Summary

This RFD introduces a configurable detached prompt policy that controls what
happens when an inquiry arrives and no interactive client is available, a
`--non-interactive` CLI flag, the `exclusive` question property, and a
four-channel output model (stdout for assistant output, stderr for chrome,
`/dev/tty` for interactive prompts, and a log file for tracing).

## Motivation

JP handles non-TTY situations today, but the behavior is implicit and not
configurable:

- Permission prompts (`RunMode::Ask|Edit`) are auto-approved when `!is_tty`.
- User-targeted tool questions are rerouted to the LLM inquiry backend.
- Result delivery prompts are auto-delivered.

This works for simple cases, but users cannot:

- Explicitly opt into non-interactive mode from a terminal (e.g., scripting).
- Choose different fallback strategies for different prompt types.
- Mark certain questions as human-only to prevent LLM auto-answering.
- Pipe JP's output cleanly (`echo "fix it" | jp query | jq` gets polluted
  with progress indicators and tool call headers).

## Design

### Detached Prompt Policy

When an inquiry (see [RFD 018]) arrives and no interactive client is attached,
JP applies the **detached prompt policy**. The policy is configurable per
inquiry kind.

Three policy modes:

| Mode       | Behavior                                                 |
|------------|----------------------------------------------------------|
| `auto`     | Auto-approve (`RunTool`/`DeliverToolResult`) or route to |
|            | LLM inquiry (`ToolQuestion`). Fails if `exclusive=true`. |
| `defaults` | Use the question's `default` value. Fail if none.        |
| `deny`     | Fail the tool call with a descriptive error.             |

Default: **`deny`**. Nothing runs unattended unless the user explicitly opts
in. This is the safe default — a non-interactive run that hits a prompt it
cannot resolve fails with a clear error message, rather than silently
auto-approving.

### The `exclusive` Property

Some tool questions cannot be meaningfully answered by the LLM:

- "Enter your SSH passphrase"
- "Confirm deletion of production database"
- "Choose which local git identity to use"

The `exclusive` property marks a question as human-only. When the detached
policy is `auto`, exclusive questions fail instead of being routed to the LLM.

At the type level, `RunTool` and `DeliverToolResult` inquiries are inherently
exclusive — this is encoded in the `Inquiry::exclusive()` method (see
[RFD 018]). `ToolQuestion` inquiries are non-exclusive by default.

Tool authors set the default:

```rust
Question {
    id: "confirm_force_push".into(),
    text: "Force push to remote?".into(),
    answer_type: AnswerType::Boolean,
    default: None,
    exclusive: true,
}
```

Users override per-question in config:

```toml
[conversation.tools.git.questions.confirm_force_push]
target = "user"
exclusive = true
```

The user has final say — they can set `exclusive = false` even for questions
the tool author marked as exclusive.

### Prompt Routing

The `route_prompt` function from [RFD 018] is extended with the detached
policy:

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

The scattered `is_tty` checks in the coordinator collapse into calls to this
function. Each call site provides the inquiry and its `exclusive` override
from config (if any).

#### Standard Input

`stdin` is exclusively for query content and context injection (e.g.,
`cat file.rs | jp query "fix this"`). It is **never** used for answering
prompts. Prompt input always comes from `/dev/tty` (interactive mode) or the
detached policy (non-interactive mode).

This means `echo "y" | jp query "do the thing"` does not answer a tool
permission prompt with "y." The "y" is treated as query content. If the query
hits a prompt and `/dev/tty` is available, the user is prompted on the
terminal. If `/dev/tty` is unavailable, the detached policy applies.

#### Determining `has_client`

`has_client` is `true` when an interactive user can answer prompts. This is
determined by:

1. If `--non-interactive` is passed, `has_client` is `false`.
2. Otherwise, JP attempts to open `/dev/tty`. If it succeeds, `has_client` is
   `true`.
3. If `/dev/tty` cannot be opened (no controlling terminal — cron, systemd,
   SSH without `-t`, daemonized processes), `has_client` is `false`.

This is independent of whether stdout is a TTY. A piped command like
`jp query | less` has stdout connected to a pipe, but `/dev/tty` is still
available because the user is at a terminal. The user can answer prompts.
Conversely, `echo foo | jp query` with stdout as a TTY might appear
interactive, but if `/dev/tty` is unavailable, it is not.

The current implementation uses `stdout.is_terminal()` as the heuristic. This
RFD replaces it with `/dev/tty` availability, which correctly handles piped
scenarios.

### Configuration

#### Scalar Shorthand

One policy for all inquiry kinds:

```toml
[conversation.tools.defaults]
detached = "deny"
```

This sets `run`, `deliver`, and `tool` to `"deny"`.

#### Per Inquiry Kind

```toml
[conversation.tools.defaults.detached]
run = "auto"
deliver = "auto"
tool = "deny"
```

The scalar-or-struct pattern follows the existing convention in JP (e.g.,
`command` accepts a string or `{ program, args, shell }`). `detached = "auto"`
is shorthand for `detached = { run = "auto", deliver = "auto", tool = "auto" }`.

#### Per Tool

```toml
[conversation.tools.fs_modify_file]
detached = "deny"

[conversation.tools.cargo_check.detached]
run = "auto"
tool = "auto"
```

#### Resolution Order

```
1. tools.<name>.detached.<kind>     (per-tool, per-kind)
2. tools.<name>.detached            (per-tool scalar)
3. tools.defaults.detached.<kind>   (global per-kind)
4. tools.defaults.detached          (global scalar)
5. "deny"                           (hardcoded safe default)
```

First match wins. This follows the same merge pattern `ToolConfigWithDefaults`
uses for `run`/`result`.

### Relationship to Existing Config

Existing config fields remain the **attached** (interactive) policies. The new
`detached` config only covers the detached case:

| Inquiry kind        | Attached policy          | Detached policy              |
|---------------------|--------------------------|------------------------------|
| `RunTool`           | `run` (ask/unattended/…) | `detached.run`               |
| `DeliverToolResult` | `result` (unattended/…)  | `detached.deliver`           |
| `ToolQuestion`      | `questions.<id>.target`  | `detached.tool` + `exclusive`|

Zero breaking changes to existing configs.

### CLI Flag

```
jp query --non-interactive "Fix the bug"
```

`--non-interactive` forces detached prompt routing even when a TTY is present.
Useful for scripting in a terminal where you don't want prompts to block.

TTY detection remains the default heuristic: when no TTY is detected, JP
behaves as if `--non-interactive` was passed.

### Output Channel Separation

JP uses four output channels, each with a single purpose:

| Channel        | Purpose                                |
|----------------|----------------------------------------|
| **stdout**     | Assistant responses, structured data   |
| **stderr**     | Chrome: progress, tool headers, status |
| **`/dev/tty`** | Interactive prompts (see below)        |
| **Log file**   | Tracing logs (`-v`)                    |

**stdout** always contains only assistant output. This makes `jp query | jq`,
`jp query > answer.txt`, and `jp query | less` work without special-casing
based on whether stdout is a TTY. When `--format json` is used, stdout
contains the structured JSON response.

**stderr** always contains chrome: progress indicators, tool call headers,
status messages. In a normal terminal session, stdout and stderr both display
on the same screen, so the user sees the same interleaved experience as today.
The separation only matters when redirecting.

**Tracing logs** (`-v` through `-vvvvv`) are written to a log file, not to
stderr. This prevents tracing output from mixing with chrome when a user
redirects stderr (e.g., `jp query 2> chrome.log` captures chrome only, not
tracing data). The log file location is configurable via `--log-file` or
`JP_LOG_FILE`, defaulting to `~/.local/share/jp/logs/`. The `--log-format`
flag controls the format of the log file (text or JSON). `--log-file=-`
writes tracing to stderr, for users who want logs and chrome on the same
stream.

This replaces the current behavior where tracing is written to stderr and all
other output goes to stdout.

**Output formatting** is controlled by `--format` and applies to both stdout
and stderr. When `--format json` is set, assistant output on stdout and chrome
on stderr are both rendered as NDJSON. When `--format auto` resolves to
`text-pretty` (stdout is a terminal), both channels use ANSI-formatted text.

Note that `stdout.is_terminal()` still controls output format resolution
(`--format auto` → `text-pretty` for terminals, `text` otherwise). This is
independent of `/dev/tty` availability, which controls interactivity
(`has_client`). The two checks serve different purposes:

- `stdout.is_terminal()` → can the consumer handle ANSI escape codes?
- `/dev/tty` available → can a user answer prompts?

### Prompt I/O Channel

Inquiry prompts (tool permissions, tool questions, result delivery
confirmations) use `/dev/tty` for both rendering and input when available.
This is the fourth output channel, independent of stdout, stderr, and the log
file.

`/dev/tty` is the controlling terminal device. It bypasses all redirections —
even `jp query > out.txt 2> err.txt` still renders prompts on the terminal.
This is the same pattern used by git (password prompts), fzf (interactive UI),
and sudo (password entry).

This means `jp query | less` works correctly: assistant output goes to `less`
via stdout, chrome goes to stderr, and tool prompts appear on the terminal via
`/dev/tty`. The channels do not interfere.

When `/dev/tty` is not available, prompts cannot be rendered and `has_client`
is `false`, so the detached policy applies.

### Integration with `Printer`

All output channels except tracing are managed through the `Printer` type.
This preserves the single-point-of-output invariant for testing and mocking.

`Printer` gains a `Tty` output target alongside the existing `Out` and `Err`:

```rust
pub enum PrintTarget {
    Out,    // stdout — assistant output, structured data
    Err,    // stderr — chrome, progress, status
    Tty,    // /dev/tty — interactive prompts
}
```

The `Tty` target:

- Always renders with ANSI (it is a terminal by definition).
- Ignores `--format` (prompts are not data output).
- Is opened lazily (first call to `tty_writer()`). Most commands never prompt,
  so `/dev/tty` is not opened unless needed.
- Returns an error if `/dev/tty` is unavailable, which feeds into
  `has_client = false`.

The `PromptBackend` trait already accepts a `writer` parameter. The change is
to wire `TerminalPromptBackend` to `printer.tty_writer()` instead of
`printer.out_writer()`. Input reading via `inquire` similarly uses `/dev/tty`
instead of stdin.

In tests, the mock `Printer` provides an in-memory buffer for the `Tty`
target, allowing prompt rendering to be asserted independently of
stdout/stderr output.

Tracing logs are handled separately via the `tracing` subscriber, configured
to write to the log file. They do not flow through `Printer`.

### Platform Portability

The `/dev/tty` and `flock` APIs are Unix-specific, but have Windows
equivalents:

| Unix             | Windows                | Rust crate                       |
|------------------|------------------------|----------------------------------|
| `/dev/tty`       | `CONIN$` / `CONOUT$`   | `crossterm`, `termwiz`           |
| `ttyname(fd)`    | Console handle detection | `$JP_SESSION` as primary        |

The `Printer` and `PromptBackend` abstractions hide the platform-specific
details. The `/dev/tty` path is an implementation detail of `tty_writer()` on
Unix; on Windows, the same method opens `CONIN$`/`CONOUT$`.

## Drawbacks

**Config surface.** The detached policy adds a new config dimension with a
scalar-or-struct pattern and four-level resolution cascade. This is powerful
but adds documentation and mental overhead.

**Breaking change in non-TTY behavior.** The current implicit behavior
(auto-approve permissions, reroute questions to LLM) is replaced by `deny` as
the default. Users who rely on the current piped behavior need to add
`detached = "auto"` to their config. This is intentional — the current
behavior is unsafe as a default — but it is a breaking change.

**Output separation changes rendering.** Chrome (progress, tool headers) goes
to stderr, assistant output goes to stdout. In a terminal, both streams
interleave on the same screen. When redirecting, the streams separate. Users
who redirect stdout to a file only see the assistant's final answer, not the
interleaved rendering they see in the terminal.

## Alternatives

### Single detached policy for all inquiry kinds

A single `detached = "auto"` covering permissions, result delivery, and tool
questions. Rejected because these are fundamentally different: auto-approving
a permission prompt (the LLM already decided to call the tool) has different
risk characteristics than auto-answering a tool question (the LLM might not
have enough context). Users need independent control.

### `exclusive` as a third `QuestionTarget` variant

Add `QuestionTarget::UserOnly` instead of a boolean flag. Rejected because
exclusivity is orthogonal to target — it describes whether the target can be
overridden when unavailable, not who the target is. At the type level,
`RunTool` and `DeliverToolResult` are inherently exclusive via the
`Inquiry::exclusive()` method. A separate `QuestionTarget` variant would not
express this.

### `auto` as the default detached policy

Default to `auto` to preserve current non-TTY behavior. Rejected because the
current behavior silently auto-approves tool execution without user consent.
`deny` is the safe default — users opt into automation explicitly.

### Environment variable instead of CLI flag

Use `JP_FRONTEND=noninteractive` (like `DEBIAN_FRONTEND`). This could be
offered as an alias, but a CLI flag is more discoverable and consistent with
JP's existing flag conventions. Both could coexist.

## Non-Goals

- **Background execution and prompt queuing.** Running conversations as
  detached background processes, the `queue` detached policy, and attach IPC
  are future work that builds on the detached policy infrastructure established
  here.
- **New inquiry variants.** This RFD uses the `Inquiry` enum from [RFD 018]
  as-is.

## Risks and Open Questions

### Interaction with the stateful tool protocol

[RFD 009] introduces stateful tools with `spawn`/`fetch`/`apply` actions.
The per-action permission model (prompt on `spawn`, auto-run `fetch`/`apply`)
maps to the detached policy, but the details need alignment during
implementation.

### Config cascade complexity

The four-level resolution is powerful but may be hard to debug. A
`jp config show --effective <tool>` command that displays the resolved detached
policy per inquiry kind would help.

### `exclusive` override direction

Users can override `exclusive = true` (set by tool authors) to `false`. This
is intentional — the user has final say — but could lead to unsafe behavior
for questions that genuinely require human judgment. Documentation should make
the implications clear.

### `result` vs `deliver` naming

The `DeliverToolResult` inquiry's config key is `deliver`, but the existing
attached config field is `result`. Options: (a) use `deliver` everywhere and
alias `result` via serde, (b) keep `result` as the config key to match the
existing field. To be resolved during implementation.

## Implementation Plan

### Phase 1: Detached Policy Config

Add the `detached` config field (scalar-or-struct) to `ToolsDefaultsConfig`
and `ToolConfig`. Implement the `DetachedMode` enum (`auto`, `defaults`,
`deny`). Implement the config resolution cascade.

Add `exclusive` field to `Question` in `jp_tool` and `QuestionConfig` in
`jp_config`.

Can be merged independently. No behavioral changes yet — the config is parsed
but not consulted.

### Phase 2: Routing Integration

Replace `is_tty` checks in the coordinator with `route_prompt()` calls that
consult the resolved detached config. Add `--non-interactive` CLI flag.

Depends on [RFD 018] (the `Inquiry` enum) and Phase 1.

### Phase 3: Output Channel Separation

Route assistant output to stdout and chrome to stderr unconditionally. Move
tracing logs from stderr to a log file. Wire `TerminalPromptBackend` to
`/dev/tty` via `Printer::tty_writer()`. Add `PrintTarget::Tty` to `Printer`.

Independent of Phases 1–2. Can be merged at any point.

## References

- [RFD 018: Typed Inquiry System](018-typed-inquiry-system.md) — the `Inquiry`
  enum this RFD's routing logic is built on.
- [RFD 009: Stateful Tool Protocol](009-stateful-tool-protocol.md) — per-action
  permission model interacts with detached policy.
- `DEBIAN_FRONTEND=noninteractive` — precedent for non-interactive policy.
- ssh `BatchMode` — precedent for "fail on prompt" policy.
- `curl` / `git` — precedent for stdout/stderr output separation.

[RFD 018]: 018-typed-inquiry-system.md
[RFD 009]: 009-stateful-tool-protocol.md
