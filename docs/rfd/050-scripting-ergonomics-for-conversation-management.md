# RFD 050: Scripting Ergonomics for Conversation Management

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Required by**: [RFD 051]
- **Summary**: Adds `jp conversation new`, a `--no-activate` flag on `jp query`,
  and shared creation flags across conversation commands for scripting.

## Summary

This RFD makes JP easier to drive from scripts and agentic workflows.
It extracts shared creation and configuration flags across conversation
commands, adds a `jp conversation new` subcommand that creates a conversation
and prints its ID, updates `jp conversation fork` to accept those flags and
print new IDs, and adds a `--no-activate` flag on `jp query` that suppresses
session updates.
Combined with `--id` ([RFD 020]), these additions let scripts and orchestrators
create, configure, and query conversations without changing the user's active
conversation.

## Motivation

JP's conversation management is designed for interactive use: `jp query` targets
the active conversation by default and activates any conversation it runs
against.
This works well for a human in a terminal, but creates two problems for scripts
and agentic workflows:

1. **Create a conversation and get its ID back.** `jp query --new` is the only
   way to start a conversation, and it immediately sends a query and activates
   the conversation.
   A script cannot pre-create a conversation for later use or pass its ID to
   another process.

2. **Operate on a conversation without side effects.** Every `jp query`
   activates the target conversation by updating either the session mapping
   ([RFD 020]) or the global `active_conversation_id` (current implementation).
   A script managing multiple conversations on behalf of a user should not
   overwrite the user's active conversation on every query.

These two primitives are independent and compose directly: a script can run `jp
conversation new` to allocate a conversation, then pass `--id` and
`--no-activate` to `jp query` to run turns without side effects.

## Design

### Shared option args

Conversation-creation flags (`--local`, `--no-local`, `--tmp`, `--title`,
`--no-title`) and config-override flags (`--model`, `--reasoning`, `--tool`, and
others) are defined directly on `Query`, making them unavailable to management
commands such as `conversation fork` and `conversation new`.
Two shared argument types expose them across commands: `ConversationCreateArgs`
and `ConversationConfigArgs`.

Flattening a single derived `#[derive(clap::Args)]` struct into each command is
not sufficient because several flag attributes vary by command:

| Concern          | Detail                                                                                                                                         |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `requires`       | `--local`, `--no-local`, and `--tmp` carry `requires = "new_conversation"` on `Query`, but are unconditional on `conversation new` and `fork`. |
| Short collisions | `Query` uses `-l` for `--local` and `-t` for `--tool`, while `conversation fork` uses `-l` for `--last` and `-t` for `--title`.                |
| `--no-local`     | Must remain mutually exclusive with `--local` and override `conversation.start_local` rather than being dropped.                               |

`crates/jp_cli/src/cmd/conversation_id.rs` already solves this for
`PositionalIds<SESSION, MULTI>` and `FlagIds<SESSION, MULTI>` by implementing
`clap::Args` manually on types parameterized by a mode marker.
The shared option args follow the same pattern.

**Semantic types.** The parsed, command-agnostic payloads:

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

`ToolDirectives` (in `cmd/query.rs`) reads clap argument indices to preserve
left-to-right ordering between `--tool` and `--no-tool`.
Moving it into a shared module is part of Phase 1; replacing it with separate
`Vec<Option<String>>` fields would lose the ordering guarantee that combinations
like `--no-tools --tool=write` rely on.

`--title` and `--no-title` apply at creation, fork, and resume time (PR #600),
so they belong on `ConversationCreateOpts` rather than on individual commands.

**Mode-parameterized wrappers.** Each command flattens a mode-typed wrapper that
parses into the semantic types:

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

Three mode markers implement `CreateArgMode`:

| Mode                  | `-l` / `-L`                       | `--title` short | `CREATE_FLAG_REQUIRES_ANY` |
| --------------------- | --------------------------------- | --------------- | -------------------------- |
| `QueryCreateMode`     | `-l` / `-L`                       | (none)          | `["new_conversation"]`     |
| `ConversationNewMode` | `-l` / `-L`                       | (none)          | `[]`                       |
| `ForkCreateMode`      | (none, `--last` owns `-l`) / `-L` | `-t`            | `[]`                       |

`ConversationConfigArgs<M: ConfigArgMode>` uses the same pattern for config
flags.
`ConfigArgMode` parameterizes the `-t` short on `--tool` and the `-a` short on
`--attachment` so `conversation fork` keeps its existing `-t` (`--title`) and
`-a` (`--activate`) short flags:

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

`augment_args` builds each clap `Arg` from the mode constants, and
`from_arg_matches` populates the shared semantic structs.
The config-application methods on `Query` (`apply_model`, `apply_reasoning`,
`apply_enable_tools`, `apply_attachments`) move onto `ConversationConfigOpts` so
all three commands share the same resolution code path.

**Flags that stay on `Query`.** Flags that only apply when executing a query
remain on `Query`: `--schema`, `--template`, `--replay`, and `--edit` /
`--no-edit`.

All three commands resolve configuration through the same layers: config files,
environment variables, `--cfg` overrides ([RFD 038]), and
`ConversationConfigArgs` flags.
The resolved config becomes the conversation's base config.

### Management commands: `conversation new` and `conversation fork`

`conversation new` and `conversation fork` create conversations for other
commands to use and share three conventions:

- **Print the new conversation ID to stdout** so scripts can pass it to `--id`.
- **Do not activate by default.** Activation is opt-in via `--activate`.
- **Accept both shared option groups** (`ConversationCreateArgs` and
  `ConversationConfigArgs`).

#### `jp conversation new`

`jp conversation new` creates an empty conversation without sending a query or
contacting the LLM, and writes only the new conversation ID to stdout:

```sh
$ jp conversation new
jp-c17528832001

$ ID="$(jp conversation new --model anthropic/claude-sonnet-4-5 --local)"
$ jp query --id="$ID" "Start working on the refactor"
```

#### `jp conversation fork`

`conversation fork` already accepts `--activate` and the turn-selector flags
(`--from`/`--to`, `--turn`, `--first`/`--last`, `--keep-first`/`--keep-last`).
This RFD adds the shared creation and configuration flags and prints the forked
conversation ID to stdout:

```sh
$ FORK_ID="$(jp conversation fork jp-c17528832001)"
$ jp query --id="$FORK_ID" "Try a different approach"

$ jp conversation fork jp-c17528832001 --model anthropic/claude-sonnet-4-5 --local
```

Config overrides are applied on top of the source conversation's base config.

`conversation fork` accepts multiple positional source conversation IDs.
When forking more than one source conversation:

- **Text output:** prints one new conversation ID per line, in the order of the
  source arguments.
- **JSON output (`-F json`):** prints a top-level array of IDs (e.g.
  `["jp-c...", "jp-c..."]`), matching JP's other list commands.
- **`--activate` with multiple sources:** rejected with an error (*"--activate
  cannot be combined with multiple source conversations; pick one to
  activate."*).
  Activating the last fork would make activation depend on argument order.

### `--no-activate` on `jp query`

```sh
jp query --id=jp-c17528832001 --no-activate "Do the thing"
```

`--no-activate` runs the query against the target conversation and persists its
events without changing the user's active conversation.
Under [RFD 020]'s session model, it skips writing the session-to-conversation
mapping file; under the global `active_conversation_id` model, it skips the
metadata update.

`--no-activate` requires `--id`, `--new`, or `--fork`.
Passing it without a conversation-targeting flag is an error because the query
would already run against the active conversation, making `--no-activate` a
no-op:

- `jp query --id=X --no-activate`: queries conversation `X` and leaves the
  active conversation unchanged.
- `jp query --new --no-activate`: creates a new conversation and queries it
  without activating it.
- `jp query --fork --no-activate`: forks the active conversation and queries the
  fork without activating it.

### Ephemeral cleanup runs only after `jp query`

`remove_ephemeral_conversations` in `crates/jp_cli/src/lib.rs` runs at the end
of every command and deletes expired, non-active ephemeral conversations.
With non-activating creation (`conversation new`, `conversation fork`) and
non-activating queries (`--no-activate`), running cleanup on every command
deletes the newly created conversation before a subsequent command can use its
ID:

```sh
ID="$(jp conversation new --tmp)"
# Without the gate, the conversation is already cleaned up here
# because `--tmp` (bare) means `expires_at = 0` and it was never activated.
jp query --id="$ID" "continue"
```

This RFD restricts `remove_ephemeral_conversations` to `jp query`; management
commands skip it.
The next `jp query` invocation still removes expired non-active ephemeral
conversations, preserving the garbage-collection contract while only changing
when cleanup runs.
`cleanup_stale_files` (orphaned locks and stale session mappings) continues to
run after every command.

Bare `--tmp` combined with `--no-activate` on `jp query` (`jp query --id=X
--no-activate --tmp "..."`) still deletes `X` at the end of that invocation
because `X` is never activated.
Workflows that reuse a non-activated ephemeral conversation across multiple
commands must pass an explicit duration (`--tmp=DURATION`).
This RFD neither changes bare `--tmp` semantics nor rejects the flag
combination, since an explicit duration directly expresses the required
lifetime.

### Command behavior summary

| Command                | Activates | Opt-out/in      | Prints ID |
| ---------------------- | --------- | --------------- | --------- |
| `jp query`             | Yes       | `--no-activate` | No        |
| `jp query --new`       | Yes       | `--no-activate` | No        |
| `jp conversation new`  | No        | `--activate`    | Yes       |
| `jp conversation fork` | No        | `--activate`    | Yes       |

Interactive commands (`query`) activate the target conversation by default
because the user is working in it.
Management commands (`conversation new`, `conversation fork`) leave activation
off by default so external callers can orchestrate conversations without side
effects.

## Drawbacks

**`conversation new` targets a narrow use case.** Interactive users rarely
create a conversation without immediately querying it, so the command exists
primarily for scripts and agentic workflows.
The maintenance cost is low: it is a thin wrapper around the existing workspace
API, and the `jp conversation` subcommand namespace is small.

## Alternatives

### Pre-generated IDs via `--id` on `jp query --new`

Instead of `jp conversation new`, allow `jp query --new --id=<pre-generated-id>`
where the script generates the ID externally:

```sh
ID="jp-c$(date +%s)0"
jp query --new --id="$ID" "Start"
```

Rejected because it leaks JP's ID format into scripts; if the format changes,
scripts break silently.
`jp conversation new` keeps ID generation internal so scripts treat the ID as
opaque.

### Derive-based shared option struct (no mode parameterization)

A simpler implementation flattens a single `#[derive(clap::Args)]` struct into
each command.
Rejected because a single derived struct cannot simultaneously preserve:

- `requires = "new_conversation"` on `--local` and `--tmp` for `Query` while
  leaving them unconditional on `conversation new` and `conversation fork`.
- `Query`'s `-l`, `-L`, and `-t` shorts where `conversation fork` has collisions
  (`-l` for `--last`, `-t` for `--title`).
- `--no-local`'s mutual exclusivity with `--local` and its override of
  `conversation.start_local`.

A derive-only approach would force users of `jp query -l` to switch to `jp query
--local`, breaking a published short flag.
The mode-parameterized pattern, already used by `PositionalIds` and `FlagIds` in
`conversation_id.rs`, preserves the existing CLI surface.

## Non-Goals

- **Background execution.** Running conversations as detached processes is
  covered by [RFD 027], which builds on the targeting primitives introduced
  here.

- **Non-interactive mode.** The `--non-interactive` flag and detached prompt
  policy are covered by [RFD 049].
  Scripts will often combine `--non-interactive` with the flags in this RFD, but
  the two concerns are orthogonal.

- **Subtree-constrained targeting.** Restricting a sub-agent to one branch of a
  conversation tree requires conversation-tree tracking and is designed
  separately.
  This RFD provides the targeting primitives (`--id`, `--no-activate`) that such
  a constraint builds on.

## Risks and Open Questions

### Exit codes

Scripts need to distinguish failure modes such as "conversation not found,"
"root-id constraint violated," and "lock timeout."
JP currently returns a single non-zero exit code for all errors.
Defining distinct exit codes is a broader CLI concern outside the scope of this
RFD.

## Implementation Plan

### Phase 1: Shared option args and fork updates

1. **Move `ToolDirectives` into a shared module.** Extract `ToolDirectives` and
   its custom clap parser from `cmd/query.rs` without changing its behavior.

2. **Introduce the mode-parameterized wrappers.** Define
   `ConversationCreateArgs<M: CreateArgMode>`, `ConversationConfigArgs<M:
   ConfigArgMode>`, the trait constants, and the mode markers
   (`QueryCreateMode`, `ConversationNewMode`, `ForkCreateMode`,
   `QueryConfigMode`, `ConversationNewConfigMode`, `ForkConfigMode`).
   Move the config application logic (`apply_model`, `apply_reasoning`,
   `apply_enable_tools`, `apply_attachments`) to methods on
   `ConversationConfigOpts`, and flatten both wrappers into `Query`.

3. **Update `conversation fork`** to flatten both wrappers, print the new
   conversation ID(s) to stdout (one per line in text mode, JSON array in `-F
   json` mode), and reject `--activate` with multiple sources.

`jp query` behavior is unchanged for existing flags.
Because the mode-trait pattern is a larger structural change than a derive-based
extraction, call it out explicitly in review.

### Phase 2: `jp conversation new`

1. **Gate `remove_ephemeral_conversations` to `jp query`.** Guard the call in
   `crates/jp_cli/src/lib.rs` with `matches!(cli.command, Commands::Query(_))`
   so `conversation new --tmp` does not clean up the conversation in the same
   invocation that produced its ID.
   `cleanup_stale_files` is unaffected.

2. **Add the `ConversationNew` subcommand.** Create a conversation using the
   shared options, activate it when `--activate` is passed, and print the ID to
   stdout.

Depends on Phase 1.

### Phase 3: `--no-activate` on `jp query`

Add the `--no-activate` flag to `Query`, requiring `--id`, `--new`, or `--fork`.
When set, skip the session mapping update ([RFD 020]) or
`active_conversation_id` update.

Can be merged independently of Phases 1 and 2.

## References

- [RFD 020: Parallel Conversations][RFD 020]: defines `--id`, `--fork`,
  session-to-conversation mapping, and conversation locks.
- [RFD 049: Non-Interactive Mode][RFD 049]: defines `--non-interactive`, often
  used alongside the scripting flags in this RFD.

[RFD 020]: 020-parallel-conversations.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 051]: 051-sub-agent-workflows-with-local-tools.md
