# RFD 049: Non-Interactive Mode and Detached Prompt Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-17
- **Required by**: [RFD 027], [RFD 051]
- **Requires**: [RFD 018], [RFD 104]
- **Summary**: Adds a `--non-interactive` flag, configurable detached prompt
  policies, and an `exclusive` question property for unattended runs.

## Summary

This RFD introduces a configurable detached prompt policy for inquiries when no
interactive client is available, a `--non-interactive` CLI flag, and an
`exclusive` question property for human-only prompts.
Together, these give users explicit control over JP's behavior in scripts, CI,
piped commands, and detached background queries.

## Motivation

Without a detached prompt policy, JP handles non-TTY runs using fixed, implicit
defaults:

- Permission prompts (`RunMode::Ask|Edit`) auto-approve when no TTY is present.
- User-targeted tool questions reroute to the LLM inquiry backend.
- Result delivery prompts auto-deliver.

These defaults cover simple cases, but users cannot:

- Explicitly opt into non-interactive mode from a terminal (for example, in
  scripts).
- Configure separate fallback strategies per prompt kind.
- Mark specific questions as human-only to prevent the LLM from answering them.

## Design

### Detached Prompt Policy

When an inquiry ([RFD 018]) arrives with no interactive client attached, JP
applies a **detached prompt policy** configured per inquiry kind:

| Mode       | Behavior                                                 |
| ---------- | -------------------------------------------------------- |
| `auto`     | Auto-approve (`RunTool`/`DeliverToolResult`) or route to |
|            | LLM inquiry (`ToolQuestion`). Fail if `exclusive=true`.  |
| `defaults` | Use the question's `default` value, or fail if none.     |
| `deny`     | Fail the tool call with a descriptive error.             |

The default mode is **`deny`**.
Nothing runs unattended unless the user explicitly opts in: a non-interactive
run that encounters an unresolved prompt fails with a descriptive error instead
of silently auto-approving.

### The `exclusive` Property

Some tool questions cannot be meaningfully answered by the LLM:

- "Enter your SSH passphrase"
- "Confirm deletion of production database"
- "Choose which local git identity to use"

The `exclusive` property marks a question as human-only.
When the detached policy is `auto`, exclusive questions fail instead of routing
to the LLM.

`Prompt::exclusive()` ([RFD 018]) encodes exclusivity at the type level:
`RunTool` and `DeliverToolResult` inquiries are inherently exclusive, whereas
`ToolQuestion` inquiries are non-exclusive by default.

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

Users can override `exclusive` per question in config:

```toml
[conversation.tools.git.questions.confirm_force_push]
target = "user"
exclusive = true
```

User configuration takes precedence, so setting `exclusive = false` overrides a
tool author's `exclusive = true`.

### Prompt Routing

`route_prompt` ([RFD 018]) takes the detached policy and optional `exclusive`
override to resolve the action when no client is attached:

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

Calls to `route_prompt` replace the scattered `is_tty` checks in the
coordinator.

### Determining `has_client`

`has_client` is `true` when an interactive user can answer prompts:

1. If `--non-interactive` is passed, `has_client` is `false`.
2. Otherwise, JP attempts to open `/dev/tty` ([RFD 048]).
   If opening `/dev/tty` succeeds, `has_client` is `true`.
3. If `/dev/tty` cannot be opened because there is no controlling terminal (such
   as in `cron`, `systemd`, SSH without `-t`, or daemonized processes),
   `has_client` is `false`.

Checking `/dev/tty` availability instead of the `stdout.is_terminal()` heuristic
decouples promptability from stdout redirection.
In a piped command such as `jp query | less`, stdout is connected to a pipe, but
`/dev/tty` remains open to the controlling terminal so the user can still answer
prompts.

### Configuration

#### Scalar Shorthand

Setting `detached` to a string applies that policy to all three prompt kinds
(`run`, `deliver`, and `tool`):

```toml
[conversation.tools.defaults]
detached = "deny"
```

#### Per Prompt Kind

`detached` also accepts a table to configure `run`, `deliver`, and `tool`
independently, following JP's scalar-or-struct convention (for example,
`command` accepts a string or `{ program, args, shell }`).
Setting `detached = "auto"` is shorthand for `detached = { run = "auto", deliver
= "auto", tool = "auto" }`:

```toml
[conversation.tools.defaults.detached]
run = "auto"
deliver = "auto"
tool = "deny"
```

#### Per Tool

Individual tools can override the default policy using either the scalar or
table form:

```toml
[conversation.tools.fs_modify_file]
detached = "deny"

[conversation.tools.cargo_check.detached]
run = "auto"
tool = "auto"
```

#### Resolution Order

JP resolves the detached policy from most specific to least specific, using the
first match (the same merge pattern `ToolConfigWithDefaults` uses for
`run`/`result`):

```txt
1. tools.<name>.detached.<kind>     (per-tool, per-kind)
2. tools.<name>.detached            (per-tool scalar)
3. tools.defaults.detached.<kind>   (global per-kind)
4. tools.defaults.detached          (global scalar)
5. "deny"                           (hardcoded safe default)
```

### Relationship to Existing Config

Existing config fields continue to govern **attached** (interactive) runs
without breaking existing configurations, while `detached` covers only the
detached case:

| Prompt kind         | Attached policy          | Detached policy               |
| ------------------- | ------------------------ | ----------------------------- |
| `RunTool`           | `run` (ask/unattended/…) | `detached.run`                |
| `DeliverToolResult` | `result` (unattended/…)  | `detached.deliver`            |
| `ToolQuestion`      | `questions.<id>.target`  | `detached.tool` + `exclusive` |

### CLI Flag

Passing `--non-interactive` forces detached prompt routing even when a TTY is
present, so scripts invoked from an interactive terminal do not block on
prompts:

```sh
jp query --non-interactive "Fix the bug"
```

TTY detection remains the default heuristic: when no TTY is detected, JP behaves
as if `--non-interactive` was passed.

## Drawbacks

**Larger config surface.** The scalar-or-struct `detached` field and its
four-level resolution cascade add documentation and mental overhead.

**Breaking change in non-TTY behavior.** Defaulting to `deny` replaces the
implicit behavior of auto-approving permissions and rerouting questions to the
LLM.
Users who rely on unattended piped behavior must add `detached = "auto"` to
their config.

## Alternatives

### Single detached policy for all inquiry kinds

A single `detached = "auto"` setting could cover permissions, result delivery,
and tool questions, but auto-approving a permission prompt carries different
risk than auto-answering a tool question.
Users need independent control over each inquiry kind.

### `auto` as the default detached policy

Defaulting to `auto` would preserve non-TTY behavior, but it silently
auto-approves tool execution without user consent.
`deny` is the safe default.

### Environment variable instead of CLI flag

An environment variable such as `JP_FRONTEND=noninteractive` (modeled on
`DEBIAN_FRONTEND`) is less discoverable than a CLI flag, though it can be added
later as an alias alongside `--non-interactive`.

## Non-Goals

- **Output channel separation.** [RFD 048] defines the four-channel output model
  (stdout, stderr, `/dev/tty`, and log file).
  This RFD consumes `/dev/tty` availability without defining the output
  architecture.

- **Background execution and prompt queuing.** Running conversations as detached
  background processes, the `queue`/`defer` detached policy, and attach IPC are
  future work.

  > [!NOTE]
  > T-1hetcnt gives a turn that a command plugin starts someone to ask: the
  > plugin.
  > It answers the prompts it can and declines the rest, and a declined prompt
  > gets the non-interactive behaviour this RFD defines.

- **New prompt variants.** This RFD uses the `Prompt` enum from [RFD 018] as-is.

## Risks and Open Questions

### Interaction with the stateful tool protocol

[RFD 009] introduces stateful tools with `spawn`/`fetch`/`apply` actions.
Its per-action permission model (prompt on `spawn`, auto-run `fetch`/`apply`)
maps to the detached policy, and the exact mapping will be aligned during
implementation.

### Config cascade complexity

Debugging a four-level resolution cascade can be difficult.
A `jp config show --effective <tool>` command that displays the resolved
detached policy per inquiry kind can make the effective configuration visible.

### `exclusive` override direction

Allowing users to override a tool author's `exclusive = true` with `exclusive =
false` is intentional, but it permits LLM auto-answering on questions that
require human judgment.
User documentation must make this risk clear.

### `result` vs `deliver` naming

The `DeliverToolResult` inquiry uses the config key `deliver` under `detached`,
whereas the attached config field is named `result`.
Whether to unify these names will be resolved during implementation.

## Implementation Plan

### Phase 1: Detached Policy Config

Add the scalar-or-struct `detached` config field to `ToolsDefaultsConfig` and
`ToolConfig`, implement the `DetachedMode` enum (`auto`, `defaults`, `deny`) and
the config resolution cascade, and add the `exclusive` field to `Question` in
`jp_tool` and `QuestionConfig` in `jp_config`.
This phase can merge independently with no behavioral changes, as the config is
parsed but not yet consulted.

### Phase 2: Routing Integration

Replace `is_tty` checks in the coordinator with `route_prompt()` calls that
consult the resolved detached config, and add the `--non-interactive` CLI flag.
This phase depends on [RFD 018] (the `Prompt` enum), [RFD 048] (`/dev/tty`
availability for `has_client`), and Phase 1.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: defines `/dev/tty` as the
  prompt I/O channel, whose availability determines `has_client`.
- [RFD 018: Typed Prompt Routing Enum][RFD 018]: defines the `Prompt` enum that
  `route_prompt` dispatches on.
- [RFD 009: Stateful Tool Protocol][RFD 009]: defines the per-action permission
  model that interacts with the detached policy.
- [RFD 019: Non-Interactive Mode][RFD 019]: the original combined RFD that this
  proposal was split from.
- `DEBIAN_FRONTEND=noninteractive`: precedent for a non-interactive policy
  setting.
- `ssh` `BatchMode`: precedent for a "fail on prompt" policy.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 051]: 051-sub-agent-workflows-with-local-tools.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
