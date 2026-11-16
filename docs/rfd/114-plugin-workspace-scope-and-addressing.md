# RFD 114: Plugin Workspace Scope and Addressing

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Requires**: [RFD 113]
- **Extends**: [RFD 072], [RFD 087]
- **Summary**: Lets command plugins declare multi-workspace scope to address
  checkouts per request, and records each turn's checkout.

## Summary

A command plugin declares in its manifest whether it operates on a single
workspace or across multiple workspaces.
Single-scope plugins receive a bootstrapped workspace from the host, while
multi-scope plugins enumerate workspaces via `list_workspaces` and name a target
checkout on every request.
Every turn also records the checkout it ran in so any frontend can resume a
conversation in the worktree that holds its edits.

## Motivation

`InitMessage.workspace` carries a single `WorkspaceInfo { root, storage, id }`,
and plugin requests carry no workspace field.
The host serves every request from the single `Workspace` resolved at bootstrap.
A plugin therefore cannot address any other workspace or discover which
workspaces exist, even though `jp_workspace::roots::known_workspaces` already
provides that list for `jp w ls`.

The web frontend (`jp-serve-web`) is a chat client: it reads and writes
conversations, edits drafts, archives conversations, sets titles, and delegates
and interrupts turns.
Switching workspaces requires quitting the host and relaunching from another
directory.
Every other long-running frontend hits the same limitation, including a TUI
dashboard, a metrics collector, or the macOS app's window-per-workspace model.

`WorkspaceInfo` also conflates a workspace with a single checkout.
One workspace ID can resolve to several live checkouts ([RFD 087]), and a
conversation can exist in only one of them: `StoragePresence::WorkspaceOnly`
describes a conversation that exists in a checkout's `.jp/conversations/`
projection and nowhere else.
Two git worktrees on different branches can hold different conversation sets,
and a plugin has no way to list or select individual checkouts.

This conflation breaks conversations that move between frontends.
If a user starts a conversation with `jp query` in one worktree and continues it
from a web frontend running in another, the next turn runs in the frontend's
checkout.
The assistant's earlier edits stay in the first worktree, its new edits land in
the second, and nothing reports an error.
Checkout addressing alone does not fix this: nothing records which checkout a
conversation last ran in, so a frontend does not know which checkout to target.

## Design

### Launch workspace vs. declared scope

Where `jp` is launched and which workspaces a plugin operates on are
independent:

- **Launch workspace**: governs plugin *admission and configuration* (which
  plugins are permitted, what `options` they receive, and what trust policy
  applies).
  The user selects the launch workspace through the `--workspace` grammar ([RFD
  087]).
- **Declared scope**: governs plugin *data access* (whether the host hands the
  plugin a single workspace at startup or the plugin names a target workspace on
  each request).

A multi-scope plugin gets its configuration from the launch workspace, but the
host does not pass that workspace to the plugin.
Running `jp -w foo serve web` configures the plugin from workspace `foo`, while
the plugin still addresses every workspace explicitly on each request.

### Declaring scope in the manifest

The plugin manifest ([RFD 072]) and the `describe` response gain a
`workspace_scope` field:

```json
{
  "command": [
    "serve",
    "web"
  ],
  "workspace_scope": "multi"
}
```

`single` is the default when `workspace_scope` is omitted, preserving existing
plugin behavior.
A plugin that declares `multi` must also require the protocol version that
introduces `workspace_scope`, so an older host rejects the plugin instead of
ignoring `multi`, bootstrapping a single workspace, and sending it in `init`.

Scope cannot be declared in the `ready` handshake message.
`ready` replies to `init`, after workspace bootstrap has already run (and
potentially prompted for a cwd-versus-active conflict or opened the workspace
picker).
A plugin that does not want a bootstrapped workspace must declare `multi` before
bootstrap runs.
The host already reads the manifest before spawning the plugin to handle command
routing and help output.

Each `workspace_scope` value maps to a `WorkspaceRequirement` level:

| Declared scope | `WorkspaceRequirement`     |
| -------------- | -------------------------- |
| `single`       | `Load`                     |
| `multi`        | a configuration-only level |

Neither existing workspace requirement fits `multi`.
`Load` and `Resolve` both fail when `jp` is invoked outside a workspace, whereas
a `multi` plugin should start regardless of the working directory.
`None` returns before the config pipeline runs and yields no configuration, but
the host still needs `plugins.command` from the config to check plugin admission
and resolve `options`.

`multi` therefore uses a configuration-only bootstrap level that resolves the
invocation's configuration ([RFD 113]) without opening a target workspace:

- **Selection.** The resolver runs every non-interactive step of [RFD 087]'s
  selection ladder: an explicit `--workspace` flag, a sticky session pin, the
  current working directory's workspace, and (when outside any workspace) a live
  session-active checkout.
  Where the ladder would prompt the user (for a cwd-versus-active conflict, the
  workspace picker, or recovery from a dead checkout), it falls back to the
  current working directory's workspace, or to the user-global config layers
  when outside a workspace.
  Prompting the user to choose a workspace makes no sense when the plugin does
  not operate on a single target workspace.
  Non-interactive invocations skip the session layer, and the resolver reads the
  session store without recording or repairing entries.
- **Failure.** Resolution does not fail solely because no workspace exists, but
  malformed or unreadable configuration files still return an error.
  Silently falling back to user-global config on a read or parse error could
  bypass a `run_policy` denial in the workspace config.
- **Side effects.** Workspace discovery is read-only: it uses
  `Workspace::open_read_only` and skips `register_checkout`, user-storage
  creation, and conversation import.
  Reusing `LoadIntent::Run` and merely skipping the conversation index is not
  sufficient, because `LoadIntent::Run` still performs those writes.

The host admits a `multi` plugin using the invocation's configuration and never
constructs a `WorkspaceCtx` for the launch workspace.

### Plugin admission order

Plugin admission checks `plugins.command` (an explicit `run = "deny"`, a pinned
checksum, or approval of a `$PATH` binary) so a rejected plugin never executes.
Checking `plugins.command` requires a resolved configuration, which requires
running bootstrap, which in turn depends on the plugin's `workspace_scope`.
Reading the embedded manifest from the binary without executing it breaks that
cycle.
Dispatch runs in a fixed order:

1. Route the invocation to a plugin binary and read its `workspace_scope` from
   the manifest.
   If an official plugin is not installed yet, download it first; downloading
   requires no configuration and executes nothing ([RFD 077]).
2. For `multi`, run the configuration-only bootstrap, admit the binary under
   that configuration, and send `init`.
3. For `single`, run the full workspace bootstrap, admit the binary under the
   selected workspace's configuration, and send `init`.

Each plugin is admitted once under the configuration that governs its
invocation, and never executes before admission succeeds.
A binary without an embedded manifest uses the manifest fields recorded when the
binary was approved ([RFD 072]).

### `InitMessage` fields

`InitMessage` splits along the same boundary that [RFD 113] uses to split `Ctx`:

| Field                                                      | `single` | `multi` |
| ---------------------------------------------------------- | -------- | ------- |
| `version`, `options`, `args`, `log_level`, `output_format` | yes      | yes     |
| `paths.user_data`, `paths.user_config`                     | yes      | yes     |
| `workspace`, `paths.user_workspace`                        | yes      | absent  |
| `config`                                                   | yes      | absent  |

`InitMessage` omits `config` for a multi-scope plugin because no single
`AppConfig` applies across all workspaces.
The host still resolves the launch workspace's configuration to extract
`options` and check admission, but does not send that `AppConfig` in `init`.
Instead, a multi-scope plugin requests configuration per checkout with
`read_config`, which runs the config pipeline for the target checkout using the
same resolution that a turn in that checkout uses ([RFD 113]).

### Checkout addressing

Every data and mutation request gains a workspace reference:

```json
{
  "type": "read_events",
  "id": "b",
  "workspace": {
    "id": "a1b2c"
  },
  "conversation": "17127583920"
}
```

```rust
pub struct WorkspaceRef {
    /// The workspace ID.
    pub id: String,

    /// Which checkout to address.
    ///
    /// Required when the workspace has more than one live checkout.
    pub root: Option<Utf8PathBuf>,
}
```

Whether `workspace` is required is determined once by `workspace_scope`:
multi-scope plugins must include `workspace` on every data and mutation request,
and single-scope plugins must omit it.
The host rejects a single-scope request that includes `workspace` or a
multi-scope request that omits it instead of falling back to the launch
workspace, which could silently route a request to the wrong workspace.

`root` selects a specific checkout when a workspace has multiple live checkouts.
A request may omit `root` only when the workspace has a single live checkout.
When the workspace has multiple live checkouts, omitting `root` returns an error
that lists the live roots, matching how `jp -w <id>` fails in non-interactive
mode ([RFD 087]).
When the workspace has no live checkouts, every request against it fails.
Defaulting to the most recently used checkout would route consecutive requests
to different checkouts whenever a concurrent `jp query` updates `last_used`, and
would run a resumed conversation in whichever worktree was touched last instead
of the worktree holding its edits.

There is no cross-root union.
A request names one checkout and sees that checkout's view: the user-local
durable store plus that root's projection.
Whether to merge a sibling worktree's conversations into a single view is a
frontend concern, not part of the protocol.

### `list_workspaces`

```json
{
  "type": "list_workspaces",
  "id": "a"
}
```

```json
{
  "type": "workspaces",
  "id": "a",
  "data": [
    {
      "id": "a1b2c",
      "slug": "jp",
      "launch": true,
      "roots": [
        {
          "path": "/Users/jean/Projects/jp",
          "storage": "/Users/jean/Projects/jp/.jp",
          "last_used": "2026-09-08T10:12:00Z"
        }
      ]
    }
  ]
}
```

`list_workspaces` returns `roots::known_workspaces` over the wire.
The `launch` flag marks the workspace `jp` was invoked against so a frontend can
highlight it as a default without re-deriving workspace selection.
A workspace whose recorded checkouts no longer exist appears with an empty
`roots` array, matching `jp w ls`.

### Tracking where a conversation last ran

Addressing lets a frontend target a specific checkout, but the frontend also
needs to know which checkout a conversation belongs to.
When the frontend creates a conversation, it already knows the target checkout;
when a conversation starts via `jp query` in a terminal, only JP knows which
checkout it ran in.

Every turn records the checkout it runs in by writing `metadata.local.json` in
the conversation's user-local directory alongside the query draft:

```json
{
  "last_root": {
    "path": "/Users/jean/Projects/jp.git/my-feature",
    "last_used": "2026-09-26T09:14:00Z"
  }
}
```

`metadata.local.json` stores machine-local conversation metadata: paths and
timestamps that are valid only on the current machine and therefore do not
belong in `metadata.json`.
`last_root` is its first field and uses the same structure as a roots registry
entry ([RFD 087]).

- **User-local only.** Storage never copies `metadata.local.json` into a
  checkout's committed projection.
  Persisting a conversation writes only the managed files to each checkout, and
  importing or synchronizing a projected conversation into user-local storage
  replaces only the managed files while preserving all other files in the
  directory, including `metadata.local.json` and the query draft.
- **Shared lifecycle with the conversation.** Renaming a conversation's title
  renames its entire user-local directory, and removing the conversation deletes
  that directory, so `metadata.local.json` moves and is deleted automatically.
- **Written when a turn first persists.** The process running the turn (`jp
  query` or the plugin host for a delegated turn) writes `last_root` when the
  turn's start event is first persisted, rather than when the conversation lock
  is acquired, because a new conversation is not written to disk until its first
  turn starts.
  A turn still running in a terminal has already persisted its start event and
  updated `last_root`.
  Because the conversation lock holder is the only writer, reading and updating
  `metadata.local.json` cannot race.
- **Forward compatible.** Every field in `metadata.local.json` is optional, and
  updates preserve unknown keys so an older `jp` binary does not strip fields
  written by a newer version.
- **Derived liveness.** The host computes `live` at read time via
  `roots::is_live` rather than storing it.
  When the recorded checkout no longer exists on disk, the host reports `live:
  false` instead of deleting the record so the frontend can tell the user that
  the conversation last ran in a removed worktree.

When user-local storage is disabled, `jp` has nowhere to store machine-local
metadata; `last_root` remains unset and the frontend prompts the user to choose
a checkout.

`list_conversations` entries and `read_events` responses include `last_root`:

```json
"last_root": {
  "path": "/Users/jean/Projects/jp.git/my-feature",
  "last_used": "2026-09-26T09:14:00Z",
  "live": true
}
```

`last_root` is omitted when no turn has recorded a checkout for the
conversation, including conversations whose last turn predates this RFD.

A frontend's cached `last_root` can become stale before the user submits a query
if a terminal turn runs in another checkout in the meantime.
To prevent a race, the host validates `last_root` under the conversation lock.
The `query` request gains an optional `expected_last_root` precondition (modeled
on the `revision` precondition in `write_draft`) containing the `last_root`
record the frontend observed:

- **Inferred continuation:** When resuming a conversation in its recorded
  checkout, the frontend sets `root` to `last_root.path` and passes the observed
  record as `expected_last_root`.
  After acquiring the conversation lock and before updating `last_root` or
  starting tools, the host compares `expected_last_root` with the stored record.
  On a mismatch, the host aborts the turn and returns a correlated error
  containing the current `last_root`.
- **Explicit checkout selection:** When the user explicitly selects a checkout,
  the frontend omits `expected_last_root`, and the host runs the turn in the
  requested checkout without checking the prior `last_root`.
- **Removed checkout (`live: false`):** The frontend warns the user that the
  previous worktree no longer exists and asks them either to pick another live
  checkout or to confirm running in the target checkout.
  Confirming sends the stale `last_root` as `expected_last_root`, which matches
  the stored record and allows the turn to run and record the new checkout.
- **No recorded checkout (`last_root` absent):** The frontend prompts the user
  to select a checkout rather than silently falling back to the launch checkout.

`last_root` is advisory metadata for frontends, not a persistent binding.
The CLI never reads `last_root`: `jp query` always resolves its target checkout
from the working directory and `--workspace`, and running a turn in another
checkout overwrites `last_root`.
Enforcing `last_root` in the CLI would require prompting whenever the working
directory differed from `last_root`.
Updating `last_root` on every CLI turn means a single `jp query` in another
worktree changes the frontend's default checkout, which is desirable because the
assistant's latest edits live in that worktree.

### The host's workspace registry

The host holds a single `CliCtx` and a map of `WorkspaceCtx` instances ([RFD
113]) keyed by checkout `(workspace_id, root)`.
Because multiple git worktrees of the same repository share a workspace ID,
keying the registry by workspace ID alone would run one worktree's turns against
another worktree's files.
The host populates the registry lazily, opening each checkout on first request
and keeping its `WorkspaceCtx` open for the life of the host process.
Checkouts stay open across requests because a delegated `query` turn spawns a
background task that holds a `ConversationLock` for the duration of the turn,
outliving the request message that started it.

Keeping checkouts open does not make reads stale: `list_conversations` and
`read_events` already refresh the conversation index on each call, so turns run
by `jp query` in another terminal appear on the next request.

MCP servers are pooled on `CliCtx` rather than per checkout.
Each turn leases server instances from the shared pool keyed by server ID,
configuration, and spawn directory ([RFD 113]).
Turns in different checkouts receive separate server instances because their
spawn directories differ, not because each checkout owns a separate pool.
A server instance starts when a turn first leases it and shuts down after an
idle timeout with no active leases; checkouts that only serve read requests
never start MCP servers.

The host tracks and releases conversation locks per checkout when the plugin
exits.

Background tasks share a single `TaskHandler` across the process (one `JoinSet`
and one cancellation token, so Ctrl-C cancels all background work at once).
Each task carries its target checkout and conversation, and `TaskHandler::sync`
resolves that checkout through the workspace registry instead of taking a single
`&mut Workspace`.
Delegated turns already persist generated titles this way through the turn's
`ConversationLock`.
Creating a separate `TaskHandler` per checkout (the alternative left open in
[RFD 113]) would duplicate `JoinSet`s and cancellation tokens without adding any
capability.

### Backward compatibility

Existing plugins (`jp-path`, `jp-ticket`, and `jp-serve-web` prior to its
multi-scope update) omit `workspace_scope`, default to `single`, and receive the
same `init` message and request schema.
Shell-script plugins continue to work without adding `workspace` fields, and
single-scope admission still uses the selected workspace's configuration.

For single-scope plugins, the only protocol additions are the `last_root` field
on conversation responses and the optional `expected_last_root` field on `query`
requests.
The host enforces the checkout check under the conversation lock even for
single-scope plugins: if `last_root` points to a different live checkout, the
host rejects the turn and names that checkout; if `last_root` points to a
removed checkout, the host rejects the turn unless `expected_last_root` confirms
it.

## Drawbacks

- **N instances of an MCP server.** A host running turns in five checkouts runs
  five instances of each server those turns use, one per spawn directory.
  Idle shutdown reclaims unused instances, but peak resource usage scales with
  the number of checkouts the frontend runs turns in.

- **Open checkout state in memory.** Each opened checkout holds a conversation
  index and file handles for the life of the host process, scaling with the
  number of distinct checkouts the frontend addresses.

- **Protocol version bump and request verbosity.** Every multi-scope data and
  mutation request must carry a `workspace` object, making hand-written JSON
  requests more verbose.
  Single-scope plugins are unaffected.

- **Scope-dependent request schema.** Whether requests carry `workspace` and
  whether `init` carries `workspace` and `config` depend on the plugin's
  declared `workspace_scope`.

## Alternatives

**Optional `workspace` field that defaults to the launch workspace.** Making
`workspace` optional on every request is strictly additive, but a multi-scope
plugin that accidentally omits `workspace` would silently act on the launch
workspace instead of failing.
Separating `single` and `multi` scopes makes that bug an immediate protocol
error.

**Require `workspace` on all requests with no single-workspace bootstrap.**
Requiring explicit addressing from every plugin breaks existing plugins and
pushes [RFD 087]'s workspace targeting grammar, session state, and conflict
prompts into each plugin.
Running `jp -w foo ticket ls` would require `jp-ticket` to resolve `foo` itself.
Host bootstrap exists so single-workspace plugins do not re-implement workspace
resolution.

**Bind each conversation to a checkout.** If a conversation permanently owned a
target root and ran every turn there regardless of the working directory,
running `jp query` in a terminal would either execute tools in a worktree the
user is not currently in or prompt on every working-directory mismatch.
Recording `last_root` as advisory metadata gives frontends the right default
checkout without changing how `jp query` behaves in a terminal.

**Store `last_root` outside `metadata.local.json`.** Storing `last_root` in a
separate user-local directory creates a second lifecycle that must be renamed,
deleted, and pruned alongside each conversation.
Recording the checkout on turn start in the event stream changes the event
schema, leaks machine-local paths into committed projections, and requires older
readers to tolerate the field.
Storing a conversation map inside each workspace roots registry entry forces
every turn in a checkout to rewrite a single shared file.
Deriving the checkout from session records fails for any conversation without a
live terminal session, including every conversation started from a frontend.

**Runtime scope switching.** A `use_workspace` message that binds the connection
to a target workspace (with an unlock message to return to explicit addressing)
fails on three counts: a plugin that only needs one non-local workspace can
already be invoked as `jp -w foo <plugin>`; connection-global state makes the
target workspace ambiguous for concurrent in-flight requests; and `use` already
refers to session-scoped workspace selection ([RFD 087]), which is a distinct
concept.
Plugins that switch workspaces at runtime can declare `multi` and pass
`workspace` on each request.

**Let plugins open workspaces directly** by linking `jp_workspace` or calling
`jp_ffi`.
Direct library access is only available to Rust and C plugins and still cannot
execute turns, which require the host's credentials, tool registry, and MCP
server pool.

**Spawn a subordinate `jp` process per workspace, proxied by the host.** Running
one child `jp` process per checkout keeps each process single-workspace and
isolates crashes, but moves workspace bootstrap semantics (targeting, session
state, and conflict prompts) out of the top-level host and spawns a full OS
process where an in-memory `WorkspaceCtx` suffices.

**Wait for [RFD 027].** A full client-server daemon architecture would subsume
multi-workspace plugin addressing, but [RFD 027] is much larger and unbuilt.
Extending the existing plugin protocol solves the problem now, and [RFD 027] can
reuse the same addressing model when it lands.

## Non-Goals

- **Authentication and authorization.** A plugin can access any workspace the
  host can reach, and a `multi` plugin can access every known workspace on the
  machine even when the web frontend binds an unauthenticated local port.
  That tradeoff is acceptable for local internal use; fine-grained plugin access
  grants are deferred to separate work.

- **Cross-root conversation views.** Each request targets a single checkout.
  Merging conversations across sibling worktrees is left to the frontend.

- **Runtime scope switching.** See Alternatives.
  Connection-level workspace binding can be added later if a concrete use case
  emerges.

- **New conversation operations.** Beyond `list_workspaces`, the `workspace`
  request field, the `last_root` metadata field, and the `expected_last_root`
  precondition, this RFD only makes existing protocol requests addressable
  across workspaces.

- **Application-layer crate extraction.** [RFD 113] defines the boundary between
  `CliCtx` and `WorkspaceCtx`; extracting those capabilities into a dedicated
  crate is separate work.

## Risks and Open Questions

- **Unbounded MCP server instances across checkouts.** A frontend that delegates
  turns across many checkouts can spawn many concurrent MCP server instances.
  The idle-shutdown policy needs a concrete default timeout and a configurable
  maximum instance count.

- **Protocol version numbering.** The protocol version counter currently
  reflects eight internal iterations that had no external consumers.
  If the protocol crate is refactored alongside this work, resetting the counter
  to version 1 would let version 1 describe the complete baseline protocol
  rather than its incremental history.

- **`launch` marker on `list_workspaces` entries.** `launch: true` exposes which
  workspace `jp` was invoked against, even though `init` otherwise hides the
  launch workspace from a `multi` plugin.
  Implementation should verify whether frontends need `launch` as a
  default-selection hint or whether omitting it keeps the `multi` contract
  cleaner.

## Implementation Plan

### Phase 1: The last-root record

- Introduce `metadata.local.json` in the conversation's user-local directory,
  preserving unknown keys on rewrite.
- Make projection sync replace only the managed files instead of the entire
  directory, so machine-local files survive a managed edit.
- Write `last_root` when a turn from `jp query` or a delegated turn first
  persists.
- Report `last_root` on `list_conversations` entries and `read_events`.
- Add `expected_last_root` to `query` and validate `last_root` under the
  conversation lock: reject the turn if `last_root` points to another live
  checkout, or if `last_root` points to a removed checkout without a matching
  `expected_last_root` confirmation.
- Update `jp-serve-web` to display removed checkouts and send the confirmation.

Independent of [RFD 113] and mergeable on its own: a web turn targeting the
wrong worktree fails with an error instead of editing the wrong worktree.

### Phase 2: Scope declaration

- Add `workspace_scope` to the plugin manifest and `describe` response,
  defaulting to `single`.
- Derive the plugin dispatch's `WorkspaceRequirement` from `workspace_scope`.
- Admit the plugin once under the configuration resolved for its declared scope.
- Omit `workspace`, `paths.user_workspace`, and `config` from `InitMessage` when
  `workspace_scope` is `multi`.
- Reject requests that include `workspace` under `single` scope or omit
  `workspace` under `multi` scope.

Depends on [RFD 113] Phase 1 and [RFD 072] Phase 5 (which introduces the plugin
manifest).
Mergeable on its own: while all plugins declare `single`, runtime behavior is
unchanged.

### Phase 3: `list_workspaces`

- Expose `roots::known_workspaces` via `list_workspaces`, including per-root
  storage paths and the `launch` marker.

Depends on Phase 2.
Mergeable on its own, allowing a frontend to list workspaces before addressing
them.

### Phase 4: Addressing and the registry

- Add `WorkspaceRef` to data and mutation requests, requiring `root` when a
  workspace has more than one live checkout.
- Populate `WorkspaceCtx` lazily per addressed checkout, and track and release
  locks per checkout.
- Attach a target checkout and conversation to each background task and resolve
  the target through the workspace registry.
- Implement the idle-shutdown policy for unleased MCP server instances.
- Update `jp-serve-web` to declare `multi`, pass `WorkspaceRef` on every
  request, resume conversations in `last_root` with `expected_last_root` set,
  prompt for a checkout when `last_root` is absent, and omit
  `expected_last_root` when the user explicitly selects a checkout.

Depends on Phases 1 and 3.

## References

- [RFD 113]: Context decomposition; defines `CliCtx`, `WorkspaceCtx`, and the
  invocation-scoped configuration used to admit `multi` plugins.
- [RFD 072]: Command plugin system; defines the plugin manifest and protocol
  that this RFD extends.
- [RFD 087]: Session-scoped active workspace; defines the roots registry,
  `known_workspaces`, the `--workspace` targeting grammar, and the entry format
  used by `last_root`.
- [RFD 031]: Durable conversation storage; defines workspace projections and
  `StoragePresence`, which make individual checkouts carry distinct conversation
  sets.
- [RFD 073]: Layered storage backend; defines the storage load path used for
  each addressed checkout.
- [RFD 077]: Plugin configuration and trust policy; defines the admission checks
  evaluated against the resolved configuration.
- [RFD 099]: Native macOS app; describes a window-per-workspace frontend that
  requires the same multi-workspace addressing model.

[RFD 027]: 027-client-server-query-architecture.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 072]: 072-command-plugin-system.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 099]: 099-native-macos-app-for-browsing-conversations.md
[RFD 113]: 113-context-decomposition-for-invocation-and-workspace-scope.md
