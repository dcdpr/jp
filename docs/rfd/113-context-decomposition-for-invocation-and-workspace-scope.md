# RFD 113: Context Decomposition for Invocation and Workspace Scope

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Extends**: [RFD 087]
- **Required by**: [RFD 114]
- **Summary**: Splits the CLI context into invocation and per-checkout workspace
  halves, resolving turn config per checkout through one pipeline.

## Summary

`jp_cli::Ctx` holds invocation state (terminal, printer, signal router, runtime)
and workspace state (workspace, config, storage backend, MCP client) in one
struct that every command borrows mutably.
This RFD splits it into `CliCtx` and `WorkspaceCtx`, routes every turn's
configuration through the one config pipeline for the checkout it runs in, and
produces the inventory of which capabilities belong to the application rather
than to a frontend.

## Motivation

`Ctx` was shaped when one `jp` run meant one workspace and one terminal.
Neither still holds.

The plugin host already broke "one terminal": a delegated turn renders to
`Printer::sink()` and pins its interrupt actions to `Stop`, because nobody is
watching the host's terminal.
It is about to break "one workspace": a plugin serving a workspace switcher
needs one bundle of workspace state per checkout it addresses, and exactly one
printer, signal router, and runtime for the process.
The unit is the checkout, not the workspace ID: two git worktrees of one
repository share a workspace ID ([RFD 087]) but hold different files, config,
and projected conversations, and a turn has to run in the right one.
Holding several `Ctx` values would duplicate the second set; threading extra
parameters through every handler would leave the two scopes entangled and the
duplication implicit.

The second consumer is the application-layer extraction.
Moving the non-frontend half of `jp_cli` into its own crate requires knowing
which side of the boundary each concern sits on.
Today that answer is not written down anywhere, and some of it is already
diverging: query-draft handling exists twice.
`jp_cli::cmd::query` resolves the draft path, fingerprints it, and removes it
conditionally; `jp_cli::cmd::plugin::dispatch` resolves the same path again with
its own `create` branch and applies a different conflict policy.
Neither lives anywhere a non-terminal frontend can reach.

If nothing changes, the workspace switcher is implementable only by threading
parameters around a god object, and the crate extraction begins by guessing at
its own surface.

## Design

This RFD is written against the tree with the in-flight plugin-host work landed:
delegated turns, query drafts over the protocol, and `list_configs`.

### Two scopes

`Ctx` becomes two types.
Every field moves to one side, except where noted below.

| Field             | Scope      | Note                                                                          |
| ----------------- | ---------- | ----------------------------------------------------------------------------- |
| `term`, `printer` | invocation | one terminal per process                                                      |
| `runtime`         | invocation | one runtime per process                                                       |
| `config`          | invocation | the bootstrap resolution, kept whole; see below                               |
| `signals`         | invocation | one signal router per process; reads the invocation's `interrupt.*`           |
| `session`         | invocation | terminal identity; its per-workspace mappings are read through `WorkspaceCtx` |
| `mcp_client`      | invocation | one pool of server instances per process; see below                           |
| `exec`            | workspace  | launch cwd, selected root, child cwd ([RFD 087])                          |
| `workspace`       | workspace  |                                                                               |
| `fs_backend`      | workspace  |                                                                               |

Commands take both.
A multi-workspace plugin host holds one `CliCtx` and a map of `WorkspaceCtx`
keyed by checkout — a workspace ID and a root — because `exec` holds exactly
one selected root.

`config_reset` and `task_handler` do not move cleanly to one side; see [Fields
that straddle](#fields-that-straddle).

### Configuration is resolved per operation

`config` stays on `CliCtx` as the whole `AppConfig` the bootstrap resolves, as
`Ctx` holds it today: loaded through [RFD 087]'s execution context — from the
launch directory when it is inside the selected checkout, and from the selected
checkout's root otherwise — with the environment and `--cfg` applied, and from
the user-global layers alone when no workspace is selected.
`jp -w B` run from inside workspace A therefore takes B's plugin admission and
`options`, as it does now.
It governs the process — plugin admission and `options`, the Ctrl-C escalation
cooldown, output format, editor settings — and for a CLI command it is also the
configuration the command runs under, conversation layer included.
It is resolved once at startup and does not change when a host opens another
checkout.
Signal handling follows from this: it is scoped to the launched `jp` process and
configured by the workspace that launched it, so a plugin can send and receive
signals but cannot configure them.

A turn a host runs on a plugin's behalf does not run under it.
It resolves its own configuration through `ConfigPipeline` for the checkout it
runs in: that checkout's files and environment as the base, loaded from its
`exec.config_cwd()` (the launch directory for the checkout `jp` was launched in,
the checkout root for any other), the conversation's stored configuration as the
per-conversation layer, then `build`.
This is the path `jp query` takes at startup.
A conversation pins its configuration, so an existing conversation resolves as
it does today; the base only supplies fields the conversation never set.
The `--cfg` arguments `jp` was launched with govern the invocation, not the
turns a host delegates, which already do not read them.

`WorkspaceCtx` therefore holds the pipeline's inputs — `workspace`,
`fs_backend`, `exec` — and no resolved configuration.
A host that stays up for hours reads a checkout's files when a turn needs them,
not when the checkout was first opened.
The resolved configuration is passed to what reads it (turn preparation, and the
tool service that leases the turn's MCP servers), so `Ctx::swap_config`, which
installs a turn's configuration into the shared context and swaps the previous
one back, is deleted.

Today the host assembles delegated-turn configuration itself: from
`load_base_partial` and `build_partial_over` for a new conversation, and from
the stored stream alone for an existing one, because the pipeline lives with the
caller that owns startup.
Putting its inputs on `WorkspaceCtx` gives the host the same pipeline the CLI
uses, and one resolution path instead of two.

### MCP servers are leased per turn

`mcp_client` is one pool per process.
It holds running server instances keyed by server name, the configuration the
instance was started with, and the directory it was spawned in, so the pool
decides what is shared, not the checkout a turn happens to run in.

A turn's tool service acquires the servers its tools name, passing the
configuration its turn resolved and its checkout's spawn directory:

- A running instance with the same key is leased to the turn.
- With no match, the pool starts a new instance, alongside any running under a
  different configuration or directory.
  A stdio server is a child process of its own, as it already is when two `jp
  query` runs in two terminals start the same server.
- The service releases its leases when the turn ends.
  An instance nobody leases is eligible to stop; how long a long-running host
  keeps one idle is the host's policy.

Acquiring a lease is asynchronous and cancellable, so a later policy can make a
turn wait for an instance without changing the interface.

Today a host shares one client across turns and starts servers by name: a
running server is reused whatever configuration started it.
A conversation pins its configuration, `providers.mcp` included, so two
conversations whose personas configure the same server differently share
whichever instance started first, and the second one's tool results come from a
server its recorded configuration does not describe.
Keying by configuration removes that.
Counting leases also removes the host's guess at whether another turn is using a
server (`McpServerScope`), which it makes today from its own list of running
turns.

A caller that names no workspace reads what it needs from `CliCtx` and
constructs no `WorkspaceCtx`.
That removes the need for a bootstrap level producing a workspace-shaped result
with no workspace in it — though `WorkspaceRequirement` ([RFD 087]) does
conflate two questions today, since `None` means "no workspace *and* no config"
by returning before the config pipeline runs.
Untangling them belongs with the first caller that needs config without a
workspace.

### Fields that straddle

**`config_reset`** is an invocation-scoped input applied to a workspace's
conversation stream.
It exists for the CLI's `--cfg` reset keywords ([RFD 038]) and has no meaning
for a caller that names no single workspace.
It lives on `CliCtx` and is refused for a multi-workspace caller.

**`task_handler`** has an invocation-scoped lifecycle and a workspace-scoped
destination.
One `JoinSet` and one cancellation token per process is right — Ctrl-C stops
all background work — but `TaskHandler::sync` takes a single `&mut Workspace`
and hands it to every drained task, and `TitleGeneratorTask::sync` silently
returns success when its conversation is not in the workspace it was given.
Drained against the wrong workspace, a generated title is dropped with no
diagnostic.

It lives on `CliCtx`, and the silent skip becomes a warning, which is the
difference between a wrong result and a visible one.
The destination problem is real but only reachable once a host addresses several
workspaces: whether to hold one handler per workspace, or one handler whose
tasks carry their own destination, is for the RFD that introduces the registry.
The single-workspace path is unaffected either way.

### What belongs to the application, and what to a frontend

The placement question for every capability is whether all three frontends —
terminal, IPC, FFI — would need it.

| Capability                              | Application | Frontend |
| --------------------------------------- | ----------- | -------- |
| Conversation read, write, lock          | yes         |          |
| Query drafts, revision conflict         | yes         |          |
| Config resolution and delta recording   | yes         |          |
| Workspace enumeration and addressing    | yes         |          |
| Turn execution                          | yes         |          |
| Typed partial-config assignment         | yes         |          |
| ANSI rendering, streaming markdown      |             | terminal |
| `KeyValueOrPath` argv grammar and clap  |             | terminal |
| JSON-lines framing, correlation IDs     |             | IPC      |
| `catch_unwind`, C strings, handle table |             | FFI      |

The last application row is the correction the inventory forces.
`PluginToHost::Query` carries `cfg: Vec<String>` parsed by the CLI's
`KeyValueOrPath`, so the IPC frontend consumes a terminal argument grammar,
including the `NONE` and `WORKSPACE` keywords whose payload is CLI-only.
A typed replacement is not a partial: one `--cfg` argument resolves to several
entries across search roots applied in precedence order, an entry can declare
`loader.reset = "none"` which becomes a reset point rather than a value, and
[RFD 070] distinguishes a named source from an anonymous assignment so that a
later `-C` can revert it.
Rather than design that surface here, the field goes away (see [Phase
3](#phase-3-the-inventory-and-the-placements-it-forces)) and returns when the
typed form is designed.

The inventory records where each capability belongs, so the later extraction is
a move rather than a redesign.
Where a capability is already on the wrong side, this RFD moves it — but only
the two Phase 3 names, not as a general licence.

### Migration mechanic

`Ctx` becomes a shell holding the two halves (`ctx.cli`, `ctx.workspace`) and
consumers migrate incrementally, one call path at a time.
The shell is deleted when the last consumer stops using it.
This keeps each step small and reviewable at the cost of both shapes existing
during the migration.

The plugin dispatch path migrates first, because a multi-workspace plugin host
depends on `WorkspaceCtx` existing and not on the rest of the CLI having moved.

## Drawbacks

- **Churn proportional to how pervasive `Ctx` is.** Every command signature
  changes.
  The work is mechanical and compiler-checked, but the diff is large and touches
  files this RFD has no other reason to open.

- **Two shapes in flight.** During the migration, some call paths take the shell
  and some take the halves.
  A reader encountering the codebase mid-flight sees an inconsistency with no
  local explanation.

- **`WorkspaceCtx` is not the library seam.** It holds the inputs to config
  resolution for the turn path, which also needs the runtime and the MCP pool on
  `CliCtx`, so it is not a type an FFI consumer can hold.
  It is the frontend-agnostic *application* bundle, and the narrower reader
  surface stays in `jp_workspace`, `jp_conversation`, and `jp_storage`.

- **The inventory is a claim, not a proof.** It is checked by the extraction
  that follows, which is where a wrong row surfaces.

## Alternatives

**Leave `Ctx` and thread parameters.** The plugin host takes the workspace
bundle as separate arguments per handler.
This works for one caller and leaves the two scopes entangled for everything
else; the duplication a multi-workspace host must avoid stays implicit, enforced
by review rather than by types.

**Hold several `Ctx` values.** Rejected: each carries a printer, a signal
router, and a runtime, of which the process needs exactly one.

**Two resolved configurations.** An invocation bundle of already-resolved
namespaces (`plugins`, `interrupt`, …) on `CliCtx`, and a resolved `AppConfig`
per checkout on `WorkspaceCtx`.
Rejected: no turn runs under a checkout's `AppConfig`, because a turn runs under
its conversation's configuration; the bundle is a new concept that forces a
namespace-by-namespace allocation; and the host's own resolution path would
remain alongside it, with `swap_config` to bridge the two.

**Extract the application crate first, and split as a consequence.** The split
is the mechanical part and the crate boundary is the irreversible one.
Doing the reversible work first means the boundary is drawn from a written
inventory rather than from whatever the compiler accepted.

## Non-Goals

- **Crate extraction.** Both halves stay in `jp_cli`.
  The application crate comes later, informed by this RFD's inventory.

- **The output and interaction inversion.** Making the turn loop emit events
  that frontends render, rather than writing to a `Printer`, is the largest
  piece of the eventual architecture and is its own RFD.
  This one only records that rendering is a frontend concern.

- **Agent loop extraction.** [RFD 026]'s territory, and it follows the output
  inversion rather than preceding it.
  Note that 026 as written extracts the loop while retaining `Printer` and moves
  `ToolRenderer` into `jp_agent`, which this RFD's inventory puts on the
  frontend side; 026 needs revision if the inversion lands first.
  Neither gates the other.

- **Multi-workspace addressing.** This RFD makes it implementable; the protocol
  surface, the host's per-checkout registry, and where background tasks deliver
  their results in it are separate work.

- **Authentication and authorization.** The plugin protocol grants a plugin
  whatever the host can reach, and the web frontend binds an unauthenticated
  port.
  That is accepted for internal use and addressed when the frontend is solid
  enough to warrant it.

## Risks and Open Questions

- **Derived state is where a silent behavior change hides.** `Ctx::new` computes
  `escalation_cooldown`, the MCP spawn directory, and the output width from its
  inputs.
  A split that recomputes any of them from a different side changes behavior
  with no test failing.
  Each derivation needs an explicit home and a test asserting the value, not
  just the wiring.

- **Resolution cost per turn.** Each delegated turn reads its checkout's config
  files and `extends` chain.
  That is what a new conversation already costs the host, and what every `jp
  query` costs at startup; a cache is a measured follow-up, and would have to
  stay fresh against file edits.

- **Servers with exclusive state.** A server that keeps state only one instance
  can hold — a database file, a browser profile, a fixed port — conflicts with
  a second instance of itself.
  Two terminals already produce that conflict; the pool makes it reachable
  inside one host.
  Declaring such a server exclusive, and making turns wait for it across
  processes, is separate work on MCP server lifecycle.

- **`session` may not be cleanly invocation-scoped.** The identity is, but every
  read of a session's active conversation goes through a workspace's mapping
  store.
  If `CliCtx` holding the identity forces workspace-shaped lookups back through
  it, the field belongs on both or the lookup belongs elsewhere.

- **Whether the inventory is complete.** It is drawn from the capabilities that
  exist today.
  A capability added between this RFD and the extraction lands on whichever side
  its author picked, unless the placement question is asked in review.

- **The `cfg` refusal has no natural end.** The protocol has no floor below
  which the host refuses a plugin, so the refusal stays until one exists or the
  protocol is renumbered with a clean break.

## Implementation Plan

### Phase 1: The two types, and the plugin dispatch path

- Introduce `CliCtx` and `WorkspaceCtx` with the field allocation above.
- `Ctx` becomes a shell holding both.
- Resolve a delegated turn's configuration through `ConfigPipeline` for its
  checkout, pass it to turn preparation explicitly, and delete
  `Ctx::swap_config`.
- Key MCP server instances in `jp_mcp::Client` by server, configuration, and
  spawn directory; lease them to a turn's tool service and route tool lookups
  and calls through the lease; delete `Ctx::configure_active_mcp_servers`, the
  per-turn `set_servers`, and `McpServerScope`.
- Warn instead of silently skipping when a background task's conversation is not
  in the workspace it was drained against.
- Migrate `run_external`, `run_plugin`, `message_loop`, and every plugin request
  handler to take the two halves.
- Assert each derived value (escalation cooldown, child cwd, output width) in a
  test.

Mergeable independently.
Unblocks multi-workspace addressing.

### Phase 2: The remaining commands

- Migrate every other command off the shell, one group at a time.
- Delete the shell.

Depends on Phase 1.
Each command group is independently mergeable.

### Phase 3: The inventory, and the placements it forces

- Record the application/frontend split as a living document under
  `docs/architecture/`.
- Consolidate query-draft handling into the storage layer: one path resolution
  instead of the two in `cmd/query.rs` and `cmd/plugin/dispatch.rs`, reachable
  by `jp query` and an FFI consumer alike.
  The two conflict policies guard different operations — the CLI refuses to
  *delete* a draft that changed since the request was composed, the plugin
  refuses to *write* over one whose revision moved — and both are kept.
- Remove `cfg` from `PluginToHost::Query` and `list_configs` from the protocol
  surface, keeping both deserializable.
  A query carrying a non-empty `cfg`, and any `list_configs` request, is
  answered with a correlated error telling the user to update the plugin, and no
  turn starts.
  A delegated turn otherwise runs under the conversation's stored configuration.
  The host's version check only refuses a plugin that needs a newer host, and
  `QueryRequest` ignores unknown fields, so without the refusal an older
  plugin's configuration selection would be dropped and its turn would succeed
  under different settings.
  A removed configuration selection must never become a successful query.
  The typed replacement takes a new field name, so an older plugin's string list
  is never read as the new form.

Depends on Phase 1.
Each item is independently mergeable and is a behavior change worth its own
review.

Removing `cfg` costs the web frontend its persona and skill selection until a
typed config-assignment surface replaces it; the CLI is unaffected.
The draft consolidation lands in `jp_storage` and `jp_workspace` and is held to
their standard, not the protocol's.

## References

- [RFD 087] — Session-scoped active workspace; defines `WorkspaceRequirement`,
  the execution context, and the roots registry.
- [RFD 072] — Command plugin system; defines the protocol whose host this RFD's
  first phase reshapes.
- [RFD 099] — Native macOS app; the FFI consumer whose constraints define the
  narrow reader surface.
- [RFD 026] — Agent loop extraction; the later boundary this RFD's inventory
  informs.
- [RFD 048] — Four-channel output model, and [RFD 012] — typed streaming
  events; the existing groundwork for treating rendering as a frontend concern.
- [RFD 038] — Config reset keywords; the origin of `config_reset`.
- [RFD 070] — Negative config deltas, and [RFD 060] — config explain; both
  consume the canonical form of a config assignment.

[RFD 012]: 012-typed-llm-streaming-events.md
[RFD 026]: 026-agent-loop-extraction.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 060]: 060-config-explain.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 072]: 072-command-plugin-system.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 099]: 099-native-macos-app-for-browsing-conversations.md
[RFD 114]: 114-plugin-workspace-scope-and-addressing.md
