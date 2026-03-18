# RFD 050: Scripting Ergonomics for Conversation Management

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17

## Summary

This RFD introduces changes to make JP easier to use in scripts and agentic
workflows: shared option structs for conversation creation and configuration, a
`jp conversation new` subcommand that creates a conversation and prints its ID,
updated `jp conversation fork` behavior to match, a `--no-activate` flag on `jp
query` that suppresses session updates, and a `--root-id` flag on `jp query`
that constrains the target to a strict descendant of a given ancestor. Together
with the `--id` flag from [RFD 020], these changes give scripts and
orchestrators precise control over conversation lifecycle and targeting without
affecting the interactive user experience.

## Motivation

JP's conversation management is designed for interactive use: `jp query` always
operates on the "active" conversation and activates whatever it touches. This
works well for a human in a terminal but creates friction for scripts and
agentic workflows that need to:

1. **Create a conversation and get its ID back.** Today, the only way to start a
   new conversation is `jp query --new`, which immediately sends a query and
   activates the conversation. A script that wants to create a conversation for
   later use, or pass the ID to another process, has no clean way to do so.

2. **Operate on a conversation without side effects.** Every `jp query`
   activates the target conversation, updating the session mapping ([RFD 020])
   or the global `active_conversation_id` (current implementation). A script
   that manages multiple conversations on behalf of a user does not want each
   query to change the user's active conversation.

3. **Constrain which conversations a sub-agent can target.** In agentic
   workflows ([RFD 040]), an orchestrator spawns sub-conversations under a
   parent. The sub-agent should only be able to target conversations within its
   assigned subtree — not the orchestrator's own conversation, and not
   conversations belonging to other sub-agents.

These are independent concerns that compose naturally: a script might use all
three (`jp conversation new` to create, `--id` to target, `--no-activate` to
avoid side effects, `--root-id` to constrain scope).

## Design

### Shared option structs

Today, conversation-creation flags (`--local`, `--tmp`) and config-override
flags (`--model`, `--reasoning`, `--tool`, etc.) live directly on the `Query`
struct. This makes them unavailable to management commands like `conversation
fork`. Two shared clap `Args` structs fix this by consolidating flags that
appear across multiple commands.

**`ConversationCreateOpts`** — flags for creating a conversation:

```rust
/// Options for creating a new conversation.
///
/// Shared between `jp query --new`, `jp conversation new`,
/// and `jp conversation fork`.
#[derive(Debug, Default, clap::Args)]
pub(crate) struct ConversationCreateOpts {
    /// Store the conversation locally, outside of the workspace.
    #[arg(short = 'l', long = "local")]
    pub local: bool,

    /// Set the expiration date of the conversation.
    #[arg(long = "tmp")]
    pub expires_in: Option<Option<humantime::Duration>>,
}
```

**`ConversationConfigOpts`** — flags that modify a conversation's config,
applicable to both new and existing conversations:

```rust
/// Config overrides applied to a conversation.
///
/// Shared between `jp query`, `jp conversation new`,
/// and `jp conversation fork`.
#[derive(Debug, Default, clap::Args)]
pub(crate) struct ConversationConfigOpts {
    /// The model to use.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// The model parameters to use.
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE",
          action = ArgAction::Append)]
    pub parameters: Vec<KvAssignment>,

    /// Enable reasoning.
    #[arg(short = 'r', long = "reasoning")]
    pub reasoning: Option<ReasoningConfig>,

    /// Disable reasoning.
    #[arg(short = 'R', long = "no-reasoning")]
    pub no_reasoning: bool,

    /// Do not display the reasoning content.
    #[arg(long = "hide-reasoning")]
    pub hide_reasoning: bool,

    /// Do not display tool calls.
    #[arg(long = "hide-tool-calls")]
    pub hide_tool_calls: bool,

    /// The tool(s) to enable.
    #[arg(short = 't', long = "tool", action = ArgAction::Append,
          num_args = 0..=1, default_missing_value = "")]
    pub tools: Vec<Option<String>>,

    /// Disable tools.
    #[arg(short = 'T', long = "no-tools", action = ArgAction::Append,
          num_args = 0..=1, default_missing_value = "")]
    pub no_tools: Vec<Option<String>>,

    /// The tool to use.
    #[arg(short = 'u', long = "tool-use")]
    pub tool_use: Option<Option<String>>,

    /// Disable tool use by the assistant.
    #[arg(short = 'U', long = "no-tool-use")]
    pub no_tool_use: bool,

    /// Add attachment to the configuration.
    #[arg(short = 'a', long = "attachment", alias = "attach")]
    pub attachments: Vec<AttachmentUrlOrPath>,
}
```

All three commands flatten both structs via `#[command(flatten)]`. The parsing
and application logic (currently in `Query::apply_cli_config`) moves to methods
on the shared structs so all commands share the same code path for config
resolution.

Flags that only make sense during an active query remain on `Query`: `--schema`,
`--template`, `--replay`, `--edit`/`--no-edit`.

Config resolution for all three commands follows the same path: file layers,
environment variables, and CLI overrides via `ConversationConfigOpts`. [RFD
038]'s `--cfg` flag applies as well for setting arbitrary config values at
creation time. The resulting config becomes the conversation's base config.

### Management commands: `conversation new` and `conversation fork`

Both `conversation new` and `conversation fork` are management commands that
create conversations for use by other commands. They share the same conventions:

- **Print the new conversation ID to stdout.** Scripts capture the ID for later
  use with `--id`.
- **Do not activate by default.** Activation is opt-in via `--activate`.
- **Accept both shared option structs** (`ConversationCreateOpts` and
  `ConversationConfigOpts`).

#### `jp conversation new`

Creates a conversation and prints its ID to stdout:

```sh
$ jp conversation new
jp-c17528832001

$ ID="$(jp conversation new --model anthropic/claude-sonnet-4-5 --local)"
$ jp query --id="$ID" "Start working on the refactor"
```

No query is sent. No LLM interaction occurs. The only output on stdout is the
conversation ID.

#### `jp conversation fork` (updated)

`conversation fork` currently accepts `--activate`, `--from`, `--until`, and
`--last` but does not support config overrides or print the new conversation ID.
This RFD adds both:

```sh
$ FORK_ID="$(jp conversation fork jp-c17528832001)"
$ jp query --id="$FORK_ID" "Try a different approach"

$ jp conversation fork jp-c17528832001 --model anthropic/claude-sonnet-4-5 --local
```

Config overrides are applied to the forked conversation's base config, on top of
whatever config the source conversation had.

### `--no-activate` on `jp query`

```sh
jp query --id=jp-c17528832001 --no-activate "Do the thing"
```

`--no-activate` suppresses the session-to-conversation mapping update ([RFD
020]) or the `active_conversation_id` update (current implementation). The query
runs against the target conversation, events are persisted, but the user's
active conversation does not change.

`--no-activate` requires one of `--id`, `--new`, or `--fork`. Using it without a
conversation-targeting flag is an error — without explicit targeting, the query
operates on the already-active conversation, making `--no-activate` a confusing
no-op.

- `jp query --id=X --no-activate`: operates on conversation X without updating
  the session mapping. The session's active conversation remains whatever it was
  before.
- `jp query --new --no-activate`: creates a new conversation, sends the query,
  but does not activate the new conversation. The session's active conversation
  remains the previous one.
- `jp query --fork --no-activate`: forks the active conversation, sends the
  query on the fork, but does not activate the fork.

Under [RFD 020]'s session model, "activating" means writing the
session-to-conversation mapping file. `--no-activate` skips this write. Under
the current global `active_conversation_id` model, it skips the metadata update.
The flag works in both models.

### `--root-id` on `jp query`

```sh
jp query --id=jp-c17528842001 --root-id=jp-c17528832001 "Continue"
```

`--root-id` constrains the target conversation to be a **strict descendant** of
the specified conversation. JP verifies the constraint before the query
executes. If the constraint is violated, the command fails with an error.

The check is strict: the target conversation must be a child, grandchild, or
deeper descendant of the root-id conversation. The target **cannot** be the
root-id conversation itself.

| Condition                                | Result             |
|------------------------------------------|--------------------|
| Target is a strict descendant of root-id | OK, query proceeds |
| Target is the root-id itself             | Error              |
| Target is not a descendant of root-id    | Error              |
| Root-id conversation does not exist      | Error              |

Error messages are specific to the failure:

```txt
Error: Conversation jp-c17528842001 is not a descendant of jp-c17528832001.
```

```txt
Error: Conversation jp-c17528832001 cannot be both the target and the
       root constraint.
```

```txt
Error: Root conversation jp-c17528832001 not found.
```

`--root-id` requires `--id`. A script using `--root-id` knows which conversation
it wants — implicit resolution via session mapping or `--last` would defeat the
purpose of the constraint.

`--root-id` is mutually exclusive with `--new` and `--fork`. It constrains
targeting of existing conversations. `--new` and `--fork` create conversations —
there is nothing to constrain.

`--root-id` requires the tree index from [RFD 039]. The ancestry check walks the
`parent_id` chain using the in-memory tree index, which is O(depth) — trivial
for realistic tree depths.

### Summary

| Command                | Activates | Opt-out/in      | Prints ID |
|------------------------|-----------|-----------------|-----------|
| `jp query`             | Yes       | `--no-activate` | No        |
| `jp query --new`       | Yes       | `--no-activate` | No        |
| `jp conversation new`  | No        | `--activate`    | Yes       |
| `jp conversation fork` | No        | `--activate`    | Yes       |

Interactive commands (`query`) activate because the user is working in that
conversation. Management commands (`conversation new`, `conversation fork`)
don't activate because the caller may be orchestrating from outside.

## Drawbacks

**`conversation new` adds a command for a narrow use case.** Interactive users
rarely need to create a conversation without querying it. The command exists
primarily for scripts and agentic workflows. However, it is small (thin wrapper
around existing workspace API) and the subcommand namespace is not crowded.

## Alternatives

### Pre-generated IDs via `--id` on `jp query --new`

Instead of `jp conversation new`, allow `jp query --new --id=<pre-generated-id>`
where the script generates the ID externally:

```sh
ID="jp-c$(date +%s)0"
jp query --new --id="$ID" "Start"
```

Rejected because it leaks JP's ID format into scripts. If the format changes,
scripts break silently. `jp conversation new` keeps ID generation internal —
scripts treat the ID as opaque.

### `--scope` or `--within` instead of `--root-id`

Alternative names for the ancestry constraint flag. `--scope` is shorter but
more abstract. `--within` reads well (`--within=<id>`) but does not convey that
the value is a conversation ID. `--root-id` is consistent with `--root` on
`conversation ls` ([RFD 039]) — both refer to tree roots — and the `-id` suffix
makes clear it takes a conversation ID.

### `--root-id` applies to `--new` / `--fork`

`--root-id` could constrain `--new` and `--fork` to create conversations *under*
the specified root, rather than being mutually exclusive. Rejected because it
would give the flag two purposes: constraining existing targets and influencing
creation. `--fork=0 --id=<parent>` ([RFD 039]) already creates a child under a
specified parent, making the creation case redundant.

## Non-Goals

- **Background execution.** Running conversations as detached processes is
  addressed by [RFD 024] and [RFD 027]. This RFD provides the targeting
  primitives that those features build on.

- **Non-interactive mode.** The `--non-interactive` flag and detached prompt
  policy are addressed by [RFD 049]. Scripts using the flags introduced here
  will often also use `--non-interactive`, but the two concerns are orthogonal.

- **Conversation access control.** `--root-id` constrains targeting based on
  tree ancestry, not permissions. It is a scoping mechanism, not a security
  boundary.

## Risks and Open Questions

### Exit codes

Scripts need to distinguish between different failure modes: "conversation not
found" vs. "root-id constraint violated" vs. "lock timeout." JP currently uses a
single non-zero exit code for all errors. A richer exit code scheme may be
needed for scripting use cases, but that is a broader concern beyond this RFD.

## Implementation Plan

### Phase 1: Shared option structs and fork updates

Extract `ConversationCreateOpts` and `ConversationConfigOpts` from `Query` into
shared structs. Move the config application logic (`apply_model`,
`apply_reasoning`, `apply_enable_tools`, `apply_attachments`) to methods on
`ConversationConfigOpts`. `Query`, `ConversationNew`, and `Fork` flatten the
shared structs.

Update `conversation fork` to accept the shared option structs and print the
forked conversation ID to stdout.

No behavioral changes to `jp query`. Can be merged independently.

### Phase 2: `jp conversation new`

Add the `ConversationNew` subcommand. It creates a conversation using the shared
options, optionally activates it, and prints the ID to stdout.

Depends on Phase 1.

### Phase 3: `--no-activate` on `jp query`

Add the `--no-activate` flag to `Query`. When set, skip the session mapping
update ([RFD 020]) or `active_conversation_id` update (current). Requires one of
`--id`, `--new`, or `--fork`.

Can be merged independently of Phase 1–2.

### Phase 4: `--root-id` on `jp query`

Add the `--root-id` flag to `Query`. Implement the strict-descendant check using
the tree index from [RFD 039]. Requires `--id`. Mutually exclusive with `--new`
and `--fork`.

Depends on [RFD 039] Phase 1 (parent_id and tree index).

## References

- [RFD 020: Parallel Conversations][RFD 020] — defines `--id`, `--fork`,
  session-to-conversation mapping, and conversation locks.
- [RFD 039: Conversation Trees][RFD 039] — defines `parent_id`, tree index, and
  `--fork=0` as child creation mechanism.
- [RFD 040: Hidden Conversations and Tool Context][RFD 040] — sub-agent
  conversations organized as children, motivating `--root-id`.
- [RFD 049: Non-Interactive Mode][RFD 049] — `--non-interactive` flag, often
  used alongside scripting flags.

[RFD 020]: 020-parallel-conversations.md
[RFD 039]: 039-conversation-trees.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
