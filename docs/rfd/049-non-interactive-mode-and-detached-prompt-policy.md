# RFD 049: Non-Interactive Mode and Detached Prompt Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-17

## Summary

This RFD introduces a configurable detached prompt policy that controls what
happens when an inquiry arrives and no interactive client is available, a
`--non-interactive` CLI flag, and the `exclusive` question property. Together
these give users explicit control over JP's behavior in non-interactive
environments — scripts, CI, piped execution, and detached background queries.

## Motivation

JP handles non-TTY situations today, but the behavior is implicit and not
configurable:

- Permission prompts (`RunMode::Ask|Edit`) are auto-approved when no TTY is
  present.
- User-targeted tool questions are rerouted to the LLM inquiry backend.
- Result delivery prompts are auto-delivered.

This works for simple cases, but users cannot:

- Explicitly opt into non-interactive mode from a terminal (e.g., scripting).
- Choose different fallback strategies for different prompt types.
- Mark certain questions as human-only to prevent LLM auto-answering.

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

Default: **`deny`**. Nothing runs unattended unless the user explicitly opts in.
This is the safe default — a non-interactive run that hits a prompt it cannot
resolve fails with a clear error message, rather than silently auto-approving.

### The `exclusive` Property

Some tool questions cannot be meaningfully answered by the LLM:

- "Enter your SSH passphrase"
- "Confirm deletion of production database"
- "Choose which local git identity to use"

The `exclusive` property marks a question as human-only. When the detached
policy is `auto`, exclusive questions fail instead of being routed to the LLM.

At the type level, `RunTool` and `DeliverToolResult` inquiries are inherently
exclusive — this is encoded in the `Prompt::exclusive()` method (see [RFD 018]).
`ToolQuestion` inquiries are non-exclusive by default.

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

The user has final say — they can set `exclusive = false` even for questions the
tool author marked as exclusive.

### Prompt Routing

The `route_prompt` function from [RFD 018] is extended with the detached policy:

```rust
fn route_prompt(
    prompt: &Prompt,
    has_client: bool,
    policy: DetachedMode,
    config_exclusive: Option<bool>,
) -> PromptAction {
    if has_client {
        return PromptAction::PromptClient;
    }

    let exclusive = config_exclusive.unwrap_or_else(|| prompt.exclusive());

    match policy {
        DetachedMode::Auto if exclusive => PromptAction::Fail,
        DetachedMode::Auto => match prompt {
            Prompt::RunTool { .. } => PromptAction::AutoApprove,
            Prompt::DeliverToolResult { .. } => PromptAction::AutoDeliver,
            Prompt::ToolQuestion { .. } => PromptAction::LlmInquiry,
        },
        DetachedMode::Defaults => PromptAction::UseDefault,
        DetachedMode::Deny => PromptAction::Fail,
    }
}
```

The scattered `is_tty` checks in the coordinator collapse into calls to this
function.

### Determining `has_client`

`has_client` is `true` when an interactive user can answer prompts. This is
determined by:

1. If `--non-interactive` is passed, `has_client` is `false`.
2. Otherwise, JP attempts to open `/dev/tty` (see [RFD 048]). If it succeeds,
   `has_client` is `true`.
3. If `/dev/tty` cannot be opened (no controlling terminal — cron, systemd, SSH
   without `-t`, daemonized processes), `has_client` is `false`.

This is independent of whether stdout is a TTY. A piped command like `jp query |
less` has stdout connected to a pipe, but `/dev/tty` is still available because
the user is at a terminal. The user can answer prompts.

The current implementation uses `stdout.is_terminal()` as the heuristic. This
RFD replaces it with `/dev/tty` availability, which correctly handles piped
scenarios.

### Configuration

#### Scalar Shorthand

One policy for all prompt kinds:

```toml
[conversation.tools.defaults]
detached = "deny"
```

This sets `run`, `deliver`, and `tool` to `"deny"`.

#### Per Prompt Kind

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

```txt
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

| Prompt kind         | Attached policy          | Detached policy              |
|---------------------|--------------------------|------------------------------|
| `RunTool`           | `run` (ask/unattended/…) | `detached.run`               |
| `DeliverToolResult` | `result` (unattended/…)  | `detached.deliver`           |
| `ToolQuestion`      | `questions.<id>.target`  | `detached.tool` + `exclusive`|

Zero breaking changes to existing configs.

### CLI Flag

```sh
jp query --non-interactive "Fix the bug"
```

`--non-interactive` forces detached prompt routing even when a TTY is present.
Useful for scripting in a terminal where you don't want prompts to block.

TTY detection remains the default heuristic: when no TTY is detected, JP behaves
as if `--non-interactive` was passed.

## Drawbacks

**Config surface.** The detached policy adds a new config dimension with a
scalar-or-struct pattern and four-level resolution cascade. This is powerful but
adds documentation and mental overhead.

**Breaking change in non-TTY behavior.** The current implicit behavior
(auto-approve permissions, reroute questions to LLM) is replaced by `deny` as
the default. Users who rely on the current piped behavior need to add `detached
= "auto"` to their config.

## Alternatives

### Single detached policy for all inquiry kinds

A single `detached = "auto"` covering permissions, result delivery, and tool
questions. Rejected because these are fundamentally different: auto-approving a
permission prompt has different risk characteristics than auto-answering a tool
question. Users need independent control.

### `auto` as the default detached policy

Default to `auto` to preserve current non-TTY behavior. Rejected because the
current behavior silently auto-approves tool execution without user consent.
`deny` is the safe default.

### Environment variable instead of CLI flag

Use `JP_FRONTEND=noninteractive` (like `DEBIAN_FRONTEND`). This could be offered
as an alias, but a CLI flag is more discoverable. Both could coexist.

## Non-Goals

- **Output channel separation.** The four-channel output model (stdout, stderr,
  `/dev/tty`, log file) is addressed in [RFD 048]. This RFD consumes `/dev/tty`
  availability but does not define the output architecture.
- **Background execution and prompt queuing.** Running conversations as detached
  background processes, the `queue`/`defer` detached policy, and attach IPC are
  future work.
- **New prompt variants.** This RFD uses the `Prompt` enum from [RFD 018] as-is.

## Risks and Open Questions

### Interaction with the stateful tool protocol

[RFD 009] introduces stateful tools with `spawn`/`fetch`/`apply` actions. The
per-action permission model (prompt on `spawn`, auto-run `fetch`/`apply`) maps
to the detached policy, but the details need alignment during implementation.

### Config cascade complexity

The four-level resolution is powerful but may be hard to debug. A `jp config
show --effective <tool>` command that displays the resolved detached policy per
inquiry kind would help.

### `exclusive` override direction

Users can override `exclusive = true` (set by tool authors) to `false`. This is
intentional but could lead to unsafe behavior for questions that genuinely
require human judgment. Documentation should make the implications clear.

### `result` vs `deliver` naming

The `DeliverToolResult` inquiry's config key is `deliver`, but the existing
attached config field is `result`. To be resolved during implementation.

## Implementation Plan

### Phase 1: Detached Policy Config

Add the `detached` config field (scalar-or-struct) to `ToolsDefaultsConfig` and
`ToolConfig`. Implement the `DetachedMode` enum (`auto`, `defaults`, `deny`).
Implement the config resolution cascade.

Add `exclusive` field to `Question` in `jp_tool` and `QuestionConfig` in
`jp_config`.

Can be merged independently. No behavioral changes yet — the config is parsed
but not consulted.

### Phase 2: Routing Integration

Replace `is_tty` checks in the coordinator with `route_prompt()` calls that
consult the resolved detached config. Add `--non-interactive` CLI flag.

Depends on [RFD 018] (the `Prompt` enum), [RFD 048] (for `/dev/tty` availability
as `has_client`), and Phase 1.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — defines `/dev/tty` as the
  prompt I/O channel; this RFD uses its availability for `has_client`.
- [RFD 018: Typed Prompt Routing Enum][RFD 018] — the `Prompt` enum this RFD's
  routing logic is built on.
- [RFD 009: Stateful Tool Protocol][RFD 009] — per-action permission model
  interacts with detached policy.
- [RFD 019: Non-Interactive Mode][RFD 019] — the original combined RFD that this
  was split from.
- `DEBIAN_FRONTEND=noninteractive` — precedent for non-interactive policy.
- ssh `BatchMode` — precedent for "fail on prompt" policy.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 048]: 048-four-channel-output-model.md
