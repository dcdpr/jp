# RFD 050: Scripting Ergonomics for Conversation Management

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Requires**: [RFD 039]
- **Required by**: [RFD 051]

## Summary

This RFD introduces changes to make JP easier to use in scripts and agentic
workflows: shared option args for conversation creation and configuration, a `jp
conversation new` subcommand that creates a conversation and prints its ID,
updated `jp conversation fork` behavior to match, a `--no-activate` flag on `jp
query` that suppresses session updates, and a `--root-id` flag on `jp query`
that constrains the target to a strict descendant of a given ancestor.
Together with the `--id` flag from [RFD 020], these changes give scripts and
orchestrators precise control over conversation lifecycle and targeting without
affecting the interactive user experience.

## Motivation

JP's conversation management is designed for interactive use: `jp query` always
operates on the "active" conversation and activates whatever it touches.
This works well for a human in a terminal but creates friction for scripts and
agentic workflows that need to:

1. **Create a conversation and get its ID back.** Today, the only way to start a
   new conversation is `jp query --new`, which immediately sends a query and
   activates the conversation.
   A script that wants to create a conversation for later use, or pass the ID to
   another process, has no clean way to do so.

2. **Operate on a conversation without side effects.** Every `jp query`
   activates the target conversation, updating the session mapping ([RFD 020])
   or the global `active_conversation_id` (current implementation).
   A script that manages multiple conversations on behalf of a user does not
   want each query to change the user's active conversation.

3. **Constrain which conversations a sub-agent can target.** In agentic
   workflows ([RFD 040]), an orchestrator spawns sub-conversations under a
   parent.
   The sub-agent should only be able to target conversations within its assigned
   subtree — not the orchestrator's own conversation, and not conversations
   belonging to other sub-agents.

These are independent concerns that compose naturally: a script might use all
three (`jp conversation new` to create, `--id` to target, `--no-activate` to
avoid side effects, `--root-id` to constrain scope).

## Design

### Shared option args

Today, conversation-creation flags (`--local`, `--no-local`, `--tmp`, `--title`,
`--no-title`) and config-override flags (`--model`, `--reasoning`, `--tool`,
etc.) live directly on the `Query` struct.
This makes them unavailable to management commands like `conversation fork`.
Two shared argument types fix this: `ConversationCreateArgs` and
`ConversationConfigArgs`.

A single derive-based `#[derive(clap::Args)]` struct flattened into each command
is **not** sufficient, because the same flag must vary per command:

| Concern          | Detail                                                                                                                                                |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requires`       | `--local` / `--no-local` / `--tmp` carry `requires = "new_conversation"` on `Query` but must be unconditional on `conversation new` and `... fork`.   |
| Short collisions | `Query` uses `-l` for `--local` and `-t` for `--tool`; `conversation fork` already uses `-l` for `--last` and `-t` for `--title`. Shorts must differ. |
| `--no-local`     | Must remain mutually exclusive with `--local` and override `conversation.start_local`. The naive proposal silently dropped this flag.                 |

The codebase already solves this kind of "same semantics, different clap
surface" problem in `crates/jp_cli/src/cmd/conversation_id.rs` with
`PositionalIds<SESSION, MULTI>` and `FlagIds<SESSION, MULTI>` — manual
`clap::Args` implementations parameterized by mode.
The shared option args follow the same pattern.

**Semantic types** — the parsed, command-agnostic payloads:

```rust
/// Options applied to a conversation at creation or resolution.
pub(crate) struct ConversationCreateOpts {
    pub locality:   Option<LocalityOverride>,    // --local / --no-local
    pub expires_in: Option<Option<Duration>>,    // --tmp[=DURATION]
    pub title:      Option<TitleOverride>,       // --title / --no-title
}

pub(crate) enum LocalityOverride { Local, Workspace }
pub(crate) enum TitleOverride    { Set(String), Clear }

/// Config overrides applied to a conversation.
pub(crate) struct ConversationConfigOpts {
    pub model:           Option<String>,
    pub parameters:      Vec<KvAssignment>,
    pub reasoning:       Option<ReasoningConfig>,
    pub no_reasoning:    bool,
    pub hide_reasoning:  bool,
    pub hide_tool_calls: bool,
    pub tool_directives: ToolDirectives,         // see below
    pub tool_use:        Option<Option<String>>,
    pub no_tool_use:     bool,
    pub attachments:     Vec<AttachmentUrlOrPath>,
}
```

`ToolDirectives` is the existing manually-parsed type from `cmd/query.rs` that
preserves left-to-right ordering between `--tool` and `--no-tool` by reading
clap argument indices.
Lifting it into a shared module is part of Phase 1; it cannot be replaced with
separate `Vec<Option<String>>` fields without losing the ordering guarantee that
compositions like `--no-tools --tool=write` rely on.

`--title` / `--no-title` apply at creation, fork, and resume time today (PR
\#600), so they belong with `ConversationCreateOpts` rather than living
separately on each command.

**Mode-parameterized wrappers** — each command flattens a mode-typed wrapper
that produces the semantic types:

```rust
pub(crate) struct ConversationCreateArgs<M: CreateArgMode> {
    opts: ConversationCreateOpts,
    _mode: PhantomData<M>,
}

pub(crate) trait CreateArgMode {
    const LOCAL_SHORT:    Option<char>;
    const NO_LOCAL_SHORT: Option<char>;
    const TITLE_SHORT:    Option<char>;
    const NO_TITLE_SHORT: Option<char>;

    /// Shared `requires_any` constraint for `--local`, `--no-local`, and
    /// `--tmp` — the three flags that today carry `requires = "new"` on
    /// `Query`. Empty means unconditional. `--title` and `--no-title` are
    /// always unconditional. Modeled as `&[&str]` (not `Option<&str>`) so
    /// future modes can require any of several flags without another
    /// refactor.
    const CREATE_FLAG_REQUIRES_ANY: &'static [&'static str];
}
```

Three mode markers:

| Mode                  | `-l` / `-L`                       | `--title` short | `CREATE_FLAG_REQUIRES_ANY` |
| --------------------- | --------------------------------- | --------------- | -------------------------- |
| `QueryCreateMode`     | `-l` / `-L`                       | (none)          | `["new_conversation"]`     |
| `ConversationNewMode` | `-l` / `-L`                       | (none)          | `[]`                       |
| `ForkCreateMode`      | (none, `--last` owns `-l`) / `-L` | `-t` (today)    | `[]`                       |

`ConversationConfigArgs<M: ConfigArgMode>` follows the same shape for the config
flags.
`ConfigArgMode` parameterizes both the `-t` short on `--tool` and the `-a` short
on `--attachment`, because `conversation fork` already uses `-t` for `--title`
*and* `-a` for `--activate`:

```rust
pub(crate) trait ConfigArgMode {
    const TOOL_SHORT:       Option<char>;
    const ATTACHMENT_SHORT: Option<char>;
}
```

| Mode                        | `--tool` short | `--attachment` short |
| --------------------------- | -------------- | -------------------- |
| `QueryConfigMode`           | `-t`           | `-a`                 |
| `ConversationNewConfigMode` | `-t`           | `-a`                 |
| `ForkConfigMode`            | (none)         | (none)               |

Do not silently steal `-a` from `--activate` or `-t` from `--title` on `fork`;
both are existing user-facing shorts.

`augment_args` builds each clap `Arg` from the mode constants;
`from_arg_matches` collects matches into the shared semantic types.
The runtime application logic (currently `apply_model`, `apply_reasoning`,
`apply_enable_tools`, `apply_attachments` on `Query`) moves to methods on the
semantic structs so all commands share the same code path.

**Flags that stay on `Query`.** Flags that only make sense during an active
query remain there: `--schema`, `--template`, `--replay`, `--edit` /
`--no-edit`.

Config resolution for all three commands follows the same path: file layers,
environment variables, and CLI overrides via `ConversationConfigArgs`.
[RFD 038]'s `--cfg` flag applies as well for setting arbitrary config values at
creation time.
The resulting config becomes the conversation's base config.

### Management commands: `conversation new` and `conversation fork`

Both `conversation new` and `conversation fork` are management commands that
create conversations for use by other commands.
They share the same conventions:

- **Print the new conversation ID to stdout.** Scripts capture the ID for later
  use with `--id`.
- **Do not activate by default.** Activation is opt-in via `--activate`.
- **Accept both shared option args** (`ConversationCreateArgs` and
  `ConversationConfigArgs`).

#### `jp conversation new`

Creates a conversation and prints its ID to stdout:

```sh
$ jp conversation new
jp-c17528832001

$ ID="$(jp conversation new --model anthropic/claude-sonnet-4-5 --local)"
$ jp query --id="$ID" "Start working on the refactor"
```

No query is sent.
No LLM interaction occurs.
The only output on stdout is the conversation ID.

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

`conversation fork` accepts multiple source conversations (positional).
When N \> 1 sources are forked:

- **Text output:** one new conversation ID per line, in the same order as the
  sources.
- **JSON output (`-F json`):** a top-level array of IDs, e.g. `["jp-c...",
  "jp-c..."]`.
  Matches the convention used elsewhere in JP for list outputs.
- **`--activate` with N \> 1 sources:** rejected with a clear error
  (*"--activate cannot be combined with multiple source conversations; pick one
  to activate."*).
  Activating "the last forked one" is too clever — its meaning would depend on
  argument order, and Hyrum's Law guarantees someone would rely on it.

### `--no-activate` on `jp query`

```sh
jp query --id=jp-c17528832001 --no-activate "Do the thing"
```

`--no-activate` suppresses the session-to-conversation mapping update ([RFD
020]) or the `active_conversation_id` update (current implementation).
The query runs against the target conversation, events are persisted, but the
user's active conversation does not change.

`--no-activate` requires one of `--id`, `--new`, or `--fork`.
Using it without a conversation-targeting flag is an error — without explicit
targeting, the query operates on the already-active conversation, making
`--no-activate` a confusing no-op.

- `jp query --id=X --no-activate`: operates on conversation X without updating
  the session mapping.
  The session's active conversation remains whatever it was before.
- `jp query --new --no-activate`: creates a new conversation, sends the query,
  but does not activate the new conversation.
  The session's active conversation remains the previous one.
- `jp query --fork --no-activate`: forks the active conversation, sends the
  query on the fork, but does not activate the fork.

Under [RFD 020]'s session model, "activating" means writing the
session-to-conversation mapping file.
`--no-activate` skips this write.
Under the current global `active_conversation_id` model, it skips the metadata
update.
The flag works in both models.

### `--root-id` on `jp query`

```sh
jp query --id=jp-c17528842001 --root-id=jp-c17528832001 "Continue"
```

`--root-id` constrains the target conversation to be a **strict descendant** of
the specified conversation.
The check is strict: the target must be a child, grandchild, or deeper
descendant.
The target **cannot** be the root-id itself.

**Enforcement timing.** The constraint lives on `ConversationLoadRequest`, not
on `Query`, so it is checked between handle resolution and per-conversation
config loading:

```
load_workspace  → load_base_partial
  → conversation_load_request           (Query produces it, with root_id)
  → resolve_request                     ← handles materialized
  → enforce_root_constraint             ← NEW; runs before config load
  → apply_conversation_config           (loads target's per-conversation config)
  → command.run
```

If the check ran inside `Query::run` instead, the merged `AppConfig` would
already contain config keys (model, system prompts, tools, ...) from a
conversation outside the allowed subtree, even if the run aborted before
persisting anything.
That undermines the scoping intent — see [Non-Goals](#non-goals) for why this
is *not* a security boundary, but it is still a correctness issue.

**Errors and precedence.** Errors are checked in this order, so the most
informative message wins:

| Order | Condition                                | Error                                                                  |
| ----- | ---------------------------------------- | ---------------------------------------------------------------------- |
| 1     | Root-id conversation does not exist      | `Root conversation <id> not found.`                                    |
| 2     | Target conversation does not exist       | (existing target-not-found error)                                      |
| 3     | Target equals root-id                    | `Conversation <id> cannot be both the target and the root constraint.` |
| 4     | Target is not a descendant of root-id    | `Conversation <target> is not a descendant of <root>.`                 |
| 5     | Target is a strict descendant of root-id | OK, query proceeds                                                     |

Existence checks must precede the equality check; otherwise the "target == root"
message is misleading when neither conversation actually exists.

**Other constraints.**

- `--root-id` requires `--id`.
  A script using `--root-id` knows which conversation it wants — implicit
  resolution via session mapping or `--last` would defeat the purpose of the
  constraint.
- `--root-id` is mutually exclusive with `--new` and `--fork`.
  It constrains targeting of existing conversations; `--new` and `--fork` create
  new ones.
- `--root-id` requires the tree index from [RFD 039].
  The ancestry check walks the `parent_id` chain using the in-memory tree index,
  which is O(depth) — trivial for realistic tree depths.

### Ephemeral cleanup runs only after `jp query`

`remove_ephemeral_conversations` in `crates/jp_cli/src/lib.rs` runs at the end
of every command today.
With non-activating creation (`conversation new`, `conversation fork`) and
non-activating queries (`--no-activate`), this races with downstream use of the
IDs the commands just produced:

```sh
ID="$(jp conversation new --tmp)"
# Without the gate, the conversation is already cleaned up here
# because `--tmp` (bare) means `expires_at = 0` and it was never activated.
jp query --id="$ID" "continue"
```

This RFD gates `remove_ephemeral_conversations` to `jp query` only.
Management commands skip it.
The next `jp query` invocation still cleans up non-active ephemerals, so the GC
contract is unchanged — only the opportunistic timing changes.
`cleanup_stale_files` (orphaned locks, stale session mappings) is not affected
and continues to run after every command.

This still leaves bare `--tmp` on `--no-activate` queries as a sharp edge: `jp
query --id=X --no-activate --tmp "..."` runs the query, never activates X, and
the same invocation cleans X up at the end.
Workflows that need to reuse a non-activated ephemeral conversation across
multiple commands must pass `--tmp=DURATION`.
The RFD does not silently change bare-`--tmp` semantics or reject the
combination — an explicit duration matches the user's actual lifetime
requirement, and rejecting compositions creates surprises elsewhere.

### Summary

| Command                | Activates | Opt-out/in      | Prints ID |
| ---------------------- | --------- | --------------- | --------- |
| `jp query`             | Yes       | `--no-activate` | No        |
| `jp query --new`       | Yes       | `--no-activate` | No        |
| `jp conversation new`  | No        | `--activate`    | Yes       |
| `jp conversation fork` | No        | `--activate`    | Yes       |

Interactive commands (`query`) activate because the user is working in that
conversation.
Management commands (`conversation new`, `conversation fork`) don't activate
because the caller may be orchestrating from outside.

## Drawbacks

**`conversation new` adds a command for a narrow use case.** Interactive users
rarely need to create a conversation without querying it.
The command exists primarily for scripts and agentic workflows.
However, it is small (thin wrapper around existing workspace API) and the
subcommand namespace is not crowded.

## Alternatives

### Pre-generated IDs via `--id` on `jp query --new`

Instead of `jp conversation new`, allow `jp query --new --id=<pre-generated-id>`
where the script generates the ID externally:

```sh
ID="jp-c$(date +%s)0"
jp query --new --id="$ID" "Start"
```

Rejected because it leaks JP's ID format into scripts.
If the format changes, scripts break silently.
`jp conversation new` keeps ID generation internal — scripts treat the ID as
opaque.

### `--scope` or `--within` instead of `--root-id`

Alternative names for the ancestry constraint flag.
`--scope` is shorter but more abstract.
`--within` reads well (`--within=<id>`) but does not convey that the value is a
conversation ID.
`--root-id` is consistent with `--root` on `conversation ls` ([RFD 039]) — both
refer to tree roots — and the `-id` suffix makes clear it takes a conversation
ID.

### `--root-id` applies to `--new` / `--fork`

`--root-id` could constrain `--new` and `--fork` to create conversations *under*
the specified root, rather than being mutually exclusive.
Rejected because it would give the flag two purposes: constraining existing
targets and influencing creation.
`--fork=0 --id=<parent>` ([RFD 039]) already creates a child under a specified
parent, making the creation case redundant.

### Derive-based shared option struct (no mode parameterization)

A simpler implementation flattens a single `#[derive(clap::Args)]` struct into
each command.
Rejected because it cannot preserve, in any combination:

- `requires = "new_conversation"` on `--local` / `--tmp` for `Query` while
  leaving them unconditional on `conversation new` and `conversation fork`.
- `Query`'s `-l` / `-L` / `-t` shorts where `conversation fork` has collisions
  (`-l` for `--last`, `-t` for `--title`).
- `--no-local`'s mutual exclusivity with `--local` and its override of
  `conversation.start_local`.

A derive-only approach would force users of `jp query -l` to switch to `jp query
--local`, which is a Hyrum's-Law breakage on a published flag short.
The mode-parameterized pattern — already used by `PositionalIds` and `FlagIds`
in `conversation_id.rs` — is the price of preserving the existing UX.

## Non-Goals

- **Background execution.** Running conversations as detached processes is
  addressed by [RFD 024] and [RFD 027].
  This RFD provides the targeting primitives that those features build on.

- **Non-interactive mode.** The `--non-interactive` flag and detached prompt
  policy are addressed by [RFD 049].
  Scripts using the flags introduced here will often also use
  `--non-interactive`, but the two concerns are orthogonal.

- **Conversation access control.** `--root-id` constrains targeting based on
  tree ancestry, not permissions.
  It is a scoping mechanism, not a security boundary.

## Risks and Open Questions

### Exit codes

Scripts need to distinguish between different failure modes: "conversation not
found" vs. "root-id constraint violated" vs. "lock timeout."
JP currently uses a single non-zero exit code for all errors.
A richer exit code scheme may be needed for scripting use cases, but that is a
broader concern beyond this RFD.

## Implementation Plan

### Phase 1: Shared option args and fork updates

Three sub-steps, in order:

1. **Lift `ToolDirectives` into a shared module.** Currently lives in
   `cmd/query.rs`.
   The custom clap parser is unchanged; only its location moves.

2. **Introduce the mode-parameterized wrappers.** Define
   `ConversationCreateArgs<M: CreateArgMode>`, `ConversationConfigArgs<M:
   ConfigArgMode>`, the trait constants, and the three mode markers
   (`QueryCreateMode`, `ConversationNewMode`, `ForkCreateMode`).
   Move the config application logic (`apply_model`, `apply_reasoning`,
   `apply_enable_tools`, `apply_attachments`) to methods on the semantic
   `ConversationConfigOpts`.
   `Query` flattens the wrappers in place of its existing fields.

3. **Update `conversation fork`** to flatten both wrappers, print the new
   conversation ID(s) to stdout (one per line in text mode, JSON array in `-F
   json` mode), and reject `--activate` with multiple sources.

No behavioral changes to `jp query` for users who don't pass new flags.
The mode-trait pattern is structurally a bigger change than a derive-based
extraction would be — flag explicitly in review.

### Phase 2: `jp conversation new`

Two sub-steps:

1. **Gate `remove_ephemeral_conversations` to `jp query`.** Today it runs at the
   end of every command in `crates/jp_cli/src/lib.rs`.
   With `conversation new --tmp` this would clean up the conversation in the
   same invocation that produced its ID.
   Move the call behind a `matches!(cli.command, Commands::Query(_))` guard.
   `cleanup_stale_files` is unaffected.

2. **Add the `ConversationNew` subcommand.** It creates a conversation using the
   shared options, optionally activates it, and prints the ID to stdout.

Depends on Phase 1.

### Phase 3: `--no-activate` on `jp query`

Add the `--no-activate` flag to `Query`.
When set, skip the session mapping update ([RFD 020]) or
`active_conversation_id` update (current).
Requires one of `--id`, `--new`, or `--fork`.

Can be merged independently of Phase 1–2.

### Phase 4: `--root-id` on `jp query`

Add the `--root-id` flag to `Query`.
Model the constraint on `ConversationLoadRequest` and enforce it between
`resolve_request` and `apply_conversation_config` so the per-conversation config
layer never loads from a conversation outside the allowed subtree (see the
Enforcement timing note in the Design section).
Requires `--id`.
Mutually exclusive with `--new` and `--fork`.

Depends on [RFD 039] Phase 1 (`parent_id` and tree index).

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
[RFD 051]: 051-sub-agent-workflows.md
