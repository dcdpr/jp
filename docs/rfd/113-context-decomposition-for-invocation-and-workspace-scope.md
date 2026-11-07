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

`jp_cli::Ctx` combines invocation state (terminal, printer, signal router,
runtime) and workspace state (workspace, config, storage backend, MCP client) in
a single struct that every command borrows mutably.
This RFD splits `Ctx` into `CliCtx` and `WorkspaceCtx`, resolves each turn's
configuration through `ConfigPipeline` for its target checkout, and defines
which capabilities belong to the application layer versus a frontend.

## Motivation

`Ctx` assumes a single `jp` process serves one workspace and one terminal.
Neither assumption still holds.

The plugin host already breaks the single-terminal assumption: a delegated turn
renders to `Printer::sink()` and pins its interrupt actions to `Stop` because
nothing watches the host's terminal.
A workspace-switching plugin also breaks the single-workspace assumption: the
host needs separate workspace state for each checkout it addresses, while
sharing a single printer, signal router, and runtime across the process.
Workspace state is scoped to a checkout, not a workspace ID: two git worktrees
of the same repository share a workspace ID ([RFD 087]) but have different
files, configuration, and projected conversations, and each turn must run in its
target checkout.
Holding multiple `Ctx` instances would duplicate the process-wide printer,
signal router, and runtime, while threading workspace parameters alongside `Ctx`
through every handler would leave invocation and workspace state entangled.

Separating the two scopes also prepares `jp_cli` for extracting an
application-layer crate.
Extracting the non-frontend logic requires a clear boundary between application
and frontend responsibilities.
Without that boundary, logic is already diverging across `jp_cli`:
`jp_cli::cmd::query` resolves the query-draft path, fingerprints the file, and
removes it conditionally, while `jp_cli::cmd::plugin::dispatch` resolves the
same path again with its own `create` branch and a different conflict policy.
Neither implementation is reachable outside `jp_cli` by a non-terminal frontend.

## Design

This design assumes the in-flight plugin-host changes have landed: delegated
turns, query drafts over the plugin protocol, and `list_configs`.

### Two scopes

`Ctx` splits into `CliCtx` (invocation scope) and `WorkspaceCtx` (workspace
scope):

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

Commands that require a workspace take both contexts.
A multi-workspace plugin host holds one `CliCtx` and a map of `WorkspaceCtx`
instances keyed by checkout (workspace ID and checkout root), because `exec`
holds a single selected root.

A caller that requires no workspace reads what it needs from `CliCtx` and
constructs no `WorkspaceCtx`.
This avoids a bootstrap stage that returns a workspace wrapper around an empty
workspace, though `WorkspaceRequirement` ([RFD 087]) still conflates two
questions because `None` means "no workspace *and* no config" by returning
before the config pipeline runs.
Separating those two cases belongs with the first caller that needs
configuration without a workspace.

`config_reset` and `task_handler` span both scopes; see [Fields that span both
scopes](#fields-that-span-both-scopes).

### Per-operation configuration resolution

`config` stays on `CliCtx` as the full `AppConfig` resolved during bootstrap.
When a workspace is selected, bootstrap loads configuration through the
execution context ([RFD 087]) from the launch directory if it lies inside the
selected checkout, or from the selected checkout's root otherwise, and then
applies environment variables and `--cfg` overrides.
Running `jp -w B` from inside workspace A therefore uses B's plugin admission
and `options`.
When no workspace is selected, bootstrap loads only the user-global layers.

This bootstrap `config` is resolved once at startup and does not change when a
plugin host opens another checkout.
It governs process-wide settings (plugin admission and `options`, the Ctrl-C
escalation cooldown, output format, and editor settings) and, for a CLI command,
is also the configuration the command runs under, including the conversation
layer.
Because signal handling is scoped to the `jp` process and configured by this
startup `config`, a plugin can send and receive signals but cannot reconfigure
them.

A delegated turn run by the plugin host does not use `CliCtx.config`.
Instead, it resolves its own configuration through `ConfigPipeline` for the
checkout it runs in, following the same path `jp query` takes at startup: it
loads the base configuration from the checkout's files and environment at
`exec.config_cwd()` (the launch directory for the startup checkout, or the
checkout root for any other checkout), layers the conversation's stored
configuration on top, and finalizes with `build`.
Because a conversation pins its configuration, the base layer only supplies
fields the conversation never set.
The `--cfg` arguments passed when launching `jp` govern the invocation, not
delegated turns.

`WorkspaceCtx` therefore holds only the pipeline's inputs (`workspace`,
`fs_backend`, and `exec`) and no resolved configuration.
A long-running host reads a checkout's configuration files when a turn starts,
not when the checkout is first opened.
Putting the pipeline inputs on `WorkspaceCtx` lets the plugin host share
`ConfigPipeline` with the CLI instead of assembling delegated-turn configuration
separately (`load_base_partial` and `build_partial_over` for a new conversation,
and the stored stream alone for an existing one).
The resolved configuration is passed directly to turn preparation and to the
tool service that leases the turn's MCP servers, so `Ctx::swap_config` (which
installed a turn's configuration into the shared context and swapped the
previous one back) is deleted.

### Per-turn MCP server leases

`mcp_client` is a single process-wide pool.
It keys running server instances by server name, the configuration the instance
was started with, and the directory it was spawned in, so the pool key
determines what is shared rather than which checkout a turn runs in.

A turn's tool service acquires leases for the servers its tools reference,
passing the turn's resolved configuration and its checkout's spawn directory:

- If a running instance matches the key, the pool leases it to the turn.
- If no instance matches, the pool starts a new instance alongside any instances
  running under a different configuration or directory.
  Each stdio server runs as its own child process, just as when two `jp query`
  invocations in separate terminals start the same server.
- When the turn ends, the tool service releases its leases.
  An instance with no active leases is eligible to stop; how long a long-running
  host keeps an idle instance running is host policy.

Acquiring a lease is asynchronous and cancellable, so a later policy can make a
turn wait for a busy instance without changing the interface.

Starting servers by name alone reuses a running instance regardless of the
configuration that started it.
Because a conversation pins its configuration (including `providers.mcp`), two
conversations whose personas configure the same server differently would share
whichever instance started first, and the second conversation's tool results
would come from a server that does not match its recorded configuration.
Keying instances by configuration prevents this mismatch.
Reference-counting leases in the pool also replaces `McpServerScope`, which
inferred whether another turn was using a server from the host's list of running
turns.

### Fields that span both scopes

**`config_reset`** is an invocation-scoped input applied to a workspace's
conversation stream.
It implements the CLI's `--cfg` reset keywords ([RFD 038]) and only applies when
a single workspace is targeted.
It stays on `CliCtx` and is rejected for multi-workspace callers.

**`task_handler`** has an invocation-scoped lifecycle and a workspace-scoped
destination.
A single `JoinSet` and cancellation token per process allow Ctrl-C to stop all
background work, so `task_handler` stays on `CliCtx`.
However, `TaskHandler::sync` takes a single `&mut Workspace` and passes it to
every drained task, and `TitleGeneratorTask::sync` silently returns success when
its conversation is not in that workspace, dropping the generated title without
a diagnostic.
This RFD changes that silent skip to a warning.
Routing background task results across multiple workspaces (either with one
handler per workspace or with tasks that carry their own destination) only
matters once a host addresses multiple workspaces, and is left to the RFD that
introduces the checkout registry.
The single-workspace path is unaffected.

### Application versus frontend capabilities

A capability belongs in the application layer if all three frontends (terminal,
IPC, and FFI) need it:

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

Typed partial-config assignment is the main boundary violation in the current
codebase.
`PluginToHost::Query` carries `cfg: Vec<String>` parsed by the CLI's
`KeyValueOrPath`, coupling the IPC frontend to a terminal argument grammar that
includes the CLI-only `NONE` and `WORKSPACE` keywords.
A typed replacement cannot simply be a partial configuration: one `--cfg`
argument can resolve to multiple entries across search roots in precedence
order, an entry can declare `loader.reset = "none"` to insert a reset point
rather than a value, and [RFD 070] distinguishes a named source from an
anonymous assignment so a later `-C` can revert it.
Rather than design that typed surface here, this RFD removes `cfg` from
`PluginToHost::Query` (see [Phase 3](#phase-3-boundary-corrections)) until the
typed form is designed.

Recording this inventory now makes the later crate extraction a mechanical move
rather than a redesign.
This RFD moves only the two capabilities listed in [Phase
3](#phase-3-boundary-corrections) (query-draft storage and protocol config
assignment), leaving the remaining moves to the extraction itself.

### Incremental migration

`Ctx` temporarily becomes a shell holding `ctx.cli` and `ctx.workspace` so
callers can migrate incrementally, one call path at a time.
The shell is deleted once the last caller stops using it.
This keeps each step small and reviewable, at the temporary cost of both context
shapes coexisting during the migration.

The plugin dispatch path migrates first, because a multi-workspace plugin host
depends on `WorkspaceCtx` existing on the plugin path and not on the remaining
CLI commands having migrated.

## Drawbacks

- **Broad signature churn.** Every command signature changes to take `CliCtx`
  and `WorkspaceCtx`.
  The migration is mechanical and compiler-checked, but produces a large diff
  across files this RFD has no other reason to touch.

- **Two context shapes during migration.** Until Phase 2 completes, some call
  paths take the `Ctx` shell and others take `CliCtx` and `WorkspaceCtx`.

- **`WorkspaceCtx` is not the FFI library seam.** It holds the inputs to turn
  configuration resolution, which also requires the runtime and MCP pool on
  `CliCtx`, so an FFI consumer cannot hold `WorkspaceCtx` on its own.
  It is the frontend-agnostic *application* bundle, while the narrower reader
  surface stays in `jp_workspace`, `jp_conversation`, and `jp_storage`.

- **The capability inventory is unverified until extraction.** A misplaced
  capability will only surface when the application crate is extracted.

## Alternatives

- **Keep `Ctx` and thread workspace parameters separately.** Passing the
  workspace bundle as extra arguments to each plugin handler works for a single
  caller, but leaves invocation and workspace state entangled everywhere else.
  Preventing process-state duplication in a multi-workspace host would rely on
  code review rather than types.

- **Hold multiple `Ctx` instances.** Rejected because each `Ctx` owns a printer,
  a signal router, and a runtime, of which the process needs exactly one.

- **Store two resolved configurations.** `CliCtx` could hold a bundle of
  resolved invocation namespaces (`plugins`, `interrupt`, and so on) while
  `WorkspaceCtx` holds a resolved `AppConfig` per checkout.
  Rejected because no turn runs under a checkout's static `AppConfig` (a turn
  runs under its conversation's configuration), the invocation bundle would
  require a new namespace-by-namespace split, and the host's custom resolution
  path would remain alongside it with `swap_config` to bridge the two.

- **Extract the application crate first, and split `Ctx` as a consequence.**
  Splitting `Ctx` is mechanical and reversible, whereas the crate boundary is
  harder to change.
  Splitting the context first ensures the crate boundary is drawn from an
  explicit inventory rather than from whatever the compiler accepted.

## Non-Goals

- **Crate extraction.** Both `CliCtx` and `WorkspaceCtx` stay in `jp_cli`.
  The application crate comes later, informed by this RFD's inventory.

- **Output and interaction inversion.** Making the turn loop emit events that
  frontends render, rather than writing directly to a `Printer`, is its own RFD.
  This RFD only records that rendering is a frontend concern.

- **Agent loop extraction ([RFD 026]).** Extracting the agent loop follows the
  output inversion.
  As written, [RFD 026] extracts the loop while retaining `Printer` and moves
  `ToolRenderer` into `jp_agent`, which this RFD's inventory places on the
  frontend side; [RFD 026] needs revision if the output inversion lands first.
  Neither RFD gates the other.

- **Multi-workspace addressing.** This RFD provides the context split needed for
  multi-workspace support, but the protocol surface, the host's per-checkout
  registry, and multi-workspace background task routing are separate work.

- **Authentication and authorization.** The plugin protocol grants a plugin
  whatever the host can reach, and the web frontend binds an unauthenticated
  port.
  This is accepted for internal use and will be addressed once the frontend is
  mature enough to warrant it.

## Risks and Open Questions

- **Silent behavior changes in derived state.** `Ctx::new` computes
  `escalation_cooldown`, the MCP spawn directory, and the output width from its
  inputs.
  Recomputing any of them from the wrong scope during the split would change
  behavior without failing existing tests.
  Each derived value needs an explicit owner and a unit test asserting its
  computed value, not just its wiring.

- **Per-turn configuration resolution cost.** Each delegated turn reads its
  checkout's configuration files and `extends` chain.
  This matches what a new conversation already costs the host and what every `jp
  query` costs at startup.
  Caching base configurations is a measured follow-up and would have to stay
  fresh against file edits.

- **MCP servers with exclusive state.** A server that holds exclusive state
  (such as a database file, a browser profile, or a fixed port) conflicts with a
  second instance of itself.
  Running `jp query` in two terminals already produces that conflict, and the
  instance pool makes it reachable inside a single host process.
  Declaring such a server exclusive and queuing turns across processes is
  separate MCP lifecycle work.

- **Scope of `session`.** The terminal session identity is invocation-scoped,
  but every read of a session's active conversation goes through a workspace's
  mapping store.
  If keeping `session` on `CliCtx` forces workspace-scoped lookups back through
  `CliCtx`, the field belongs on both contexts or the lookup belongs elsewhere.

- **Completeness of the capability inventory.** The inventory covers the
  capabilities that exist today.
  Any capability added before the crate extraction must be evaluated against the
  same application-versus-frontend criterion in review.

- **Lifetime of the `cfg` rejection.** The plugin protocol has no minimum
  version floor below which the host refuses a plugin, so the host must keep
  rejecting `cfg` and `list_configs` until a version floor exists or the
  protocol makes a breaking version bump.

## Implementation Plan

### Phase 1: Context split and plugin dispatch

- Introduce `CliCtx` and `WorkspaceCtx` with the field allocation above.
- Turn `Ctx` into a shell holding both contexts.
- Resolve a delegated turn's configuration through `ConfigPipeline` for its
  checkout, pass it to turn preparation explicitly, and delete
  `Ctx::swap_config`.
- Key MCP server instances in `jp_mcp::Client` by server, configuration, and
  spawn directory; lease them to a turn's tool service and route tool lookups
  and calls through the lease; delete `Ctx::configure_active_mcp_servers`, the
  per-turn `set_servers`, and `McpServerScope`.
- Log a warning instead of silently skipping when a background task's
  conversation is not in the workspace it was drained against.
- Migrate `run_external`, `run_plugin`, `message_loop`, and every plugin request
  handler to take `CliCtx` and `WorkspaceCtx`.
- Add unit tests asserting each derived value (escalation cooldown, child cwd,
  and output width).

Phase 1 is independently mergeable and unblocks multi-workspace addressing.

### Phase 2: Remaining CLI commands

- Migrate every remaining command off the `Ctx` shell, one command group at a
  time.
- Delete the `Ctx` shell.

Phase 2 depends on Phase 1; each command group is independently mergeable.

### Phase 3: Boundary corrections

- Record the application-versus-frontend split as a living document under
  `docs/wiki/`.
- Consolidate query-draft handling in `jp_storage` and `jp_workspace`: replace
  the duplicate path resolutions in `cmd/query.rs` and `cmd/plugin/dispatch.rs`
  with a single implementation reachable by `jp query` and FFI consumers alike,
  designed to `jp_storage` and `jp_workspace` conventions rather than the plugin
  protocol's.
  Both conflict policies remain because they guard different operations: the CLI
  refuses to *delete* a draft that changed after the request was composed, and
  the plugin host refuses to *write* over a draft whose revision moved.
- Remove `cfg` from `PluginToHost::Query` and `list_configs` from the protocol
  surface while keeping both deserializable.
  Any query carrying a non-empty `cfg` and any `list_configs` request is
  answered with a correlated error instructing the user to update the plugin,
  and no turn starts; otherwise, a delegated turn runs under the conversation's
  stored configuration.
  The host's version check only refuses a plugin that requires a newer host, and
  `QueryRequest` ignores unknown fields, so dropping `cfg` from the wire schema
  would silently ignore an older plugin's configuration selection and run its
  turn under different settings.
  The future typed replacement will use a new field name so an older plugin's
  string list is never deserialized as the new form.
  Removing `cfg` temporarily disables persona and skill selection in the web
  frontend until that typed config-assignment surface replaces it; the CLI is
  unaffected.

Phase 3 depends on Phase 1.
Each item is a standalone behavior change that can merge and be reviewed
independently.

## References

- [RFD 087]: Session-scoped active workspace; defines `WorkspaceRequirement`,
  the execution context, and the roots registry.
- [RFD 072]: Command plugin system; defines the protocol whose host Phase 1
  reshapes.
- [RFD 099]: Native macOS app; the FFI consumer whose constraints define the
  narrow reader surface.
- [RFD 026]: Agent loop extraction; the later boundary informed by this RFD's
  inventory.
- [RFD 048] (four-channel output model) and [RFD 012] (typed streaming events):
  the existing groundwork for treating rendering as a frontend concern.
- [RFD 038]: Config reset keywords; introduces `config_reset`.
- [RFD 070] (negative config deltas) and [RFD 060] (config explain): both
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
