# RFD 114: Plugin Workspace Scope and Addressing

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Requires**: [RFD 113]
- **Extends**: [RFD 072], [RFD 087]
- **Summary**: Command plugins declare single- or multi-workspace scope;
  multi-scope plugins address a checkout per request, and turns record their
  checkout.

## Summary

A command plugin declares in its self-description whether it operates on one
workspace or on several.
A single-scope plugin gets today's behavior: the host bootstraps a workspace and
every request is about it.
A multi-scope plugin gets no bootstrapped workspace, enumerates workspaces with
`list_workspaces`, and names one checkout on every request.
Every turn records the checkout it ran in, so a frontend can continue a
conversation started in a terminal in the tree that holds its edits.

## Motivation

`InitMessage.workspace` carries one `WorkspaceInfo { root, storage, id }`, and
no request carries a workspace field.
The host answers all of them from the single `Workspace` its bootstrap resolved.
A plugin therefore cannot address a workspace other than the one `jp` was
launched against, and cannot discover which workspaces exist — even though
`jp_workspace::roots::known_workspaces` already answers that question for `jp w
ls`.

The web frontend is a chat client: it reads and writes conversations, edits
drafts, archives, sets titles, delegates turns, and interrupts them.
Switching workspaces in it means quitting the host and relaunching from another
directory.
The same wall stands in front of every other long-running frontend — a TUI
dashboard, a metrics collector, the macOS app's window-per-workspace model.

`WorkspaceInfo` also flattens a second thing.
One workspace ID resolves to several live checkouts ([RFD 087]), and a
conversation can be present in only one of them:
`StoragePresence::WorkspaceOnly` describes a conversation that exists in a
checkout's `.jp/conversations/` projection and nowhere else.
Two git worktrees on different branches genuinely hold different conversation
sets, and a plugin has no way to see or choose between them.

The flattening does damage when a conversation moves between frontends.
A conversation started with `jp query` in one worktree and continued from a web
frontend running in another runs its next turn in the frontend's checkout: the
assistant's earlier edits are in one tree, its new ones land in the other, and
nothing reports an error.
Addressing alone does not fix this, because nothing records which checkout a
conversation last ran in, and a frontend cannot name a checkout it does not
know.

Doing nothing leaves the frontend that most needs to browse across workspaces
unable to, and leaves each new frontend rediscovering the same limit.

## Design

### Two axes

Where `jp` was launched and what a plugin operates on are independent:

- **Launch workspace** — governs plugin *admission and configuration*.
  Which plugins are permitted, what `options` they receive, what trust policy
  applies.
  Selected by the user through the full `--workspace` grammar ([RFD 087]).
- **Declared scope** — governs plugin *data access*.
  Whether the plugin is handed one workspace or names one per request.

A multi-scope plugin is configured by the launch workspace and told nothing
about it.
`jp -w foo serve web` configures the plugin from workspace `foo`; the plugin
still addresses every workspace explicitly.

### Scope is declared in `describe`, not at handshake

`DescribeResponse` gains a scope field:

```json
{
  "type": "describe",
  "name": "serve-web",
  "command": [
    "serve",
    "web"
  ],
  "workspace_scope": "multi"
}
```

`single` is the default and the wire default, so a plugin that says nothing
behaves exactly as it does today.

The declaration cannot live in `ready`.
`ready` answers `init`, and by then the bootstrap has already run — possibly
prompting the cwd-versus-active conflict, possibly opening the picker.
A plugin declaring it wants no bootstrapped workspace has to say so before that
happens.
The host already consults `describe` for routing and help, which is why it is
the right place.

The declaration feeds the existing bootstrap declaration mechanism:

| Declared scope | `WorkspaceRequirement`     |
| -------------- | -------------------------- |
| `single`       | `Load`                     |
| `multi`        | a configuration-only level |

`multi` needs a level that does not exist yet.
`Load` and `Resolve` both fail outside a workspace, and a plugin that declared
it does not care where it was launched from should not fail because of where it
was launched from.
`None` returns before the config pipeline runs, so it yields no configuration —
and plugin admission and `options` come from `plugins.command`, which means
configuration is exactly what `multi` still needs.

The level therefore resolves the invocation's configuration ([RFD 113]) without
resolving a workspace to operate on.
Three things it has to pin down, because the existing resolver does more than
cwd discovery:

- **Selection.** Every step of [RFD 087]'s ladder that asks nothing: an explicit
  `--workspace`, a sticky session pin, the cwd's workspace, and the
  session-active workspace outside any workspace while its checkout is live.
  Where the ladder would prompt — the cwd-versus-active conflict, the picker,
  recovery from a dead checkout — it takes the cwd's workspace instead, or the
  user-global layers when there is none.
  A recorded selection is what today's admission already reads; only a prompt
  asks a caller that operates on no workspace a question with no consequence it
  can reason about.
  Non-interactive runs skip the session layer, as they do today, and the level
  reads the session store without recording or repairing anything.
- **Failure.** It does not fail *solely because no workspace exists*.
  Malformed or unreadable configuration remains an error.
  Silently degrading to the user-global layers would turn a workspace's
  `run_policy` denial into a permission, and the user would see success.
- **Side effects.** Discovery is observational: `Workspace::open_read_only`, no
  `register_checkout`, no user-storage creation, no conversation import.
  `LoadIntent::Run` does all of those, so reusing the ordinary opening path and
  merely skipping the index is not configuration-only.

A `multi` plugin is admitted from the invocation's configuration and never
receives a `WorkspaceCtx`, so nothing has to fabricate a root for it.

### Admission before description

Answering `describe` means running the plugin, and admission — an explicit `run
= "deny"`, a pinned checksum, approval of a `$PATH` binary — exists so a plugin
the configuration refuses never runs.
Admission reads `plugins.command`, which needs a configuration, which needs a
bootstrap, and the bootstrap depends on the scope `describe` reports.
The dispatch breaks that loop in a fixed order:

1. Resolve the configuration-only level above: every step of the ladder that
   asks nothing, and the cwd's workspace where it would prompt.
2. Admit the binary under that configuration.
   A plugin denied or unapproved here is not run at all.
3. Send `describe` and read `workspace_scope`.
4. For `multi`, send `init` with that configuration.
5. For `single`, run the full workspace bootstrap, admit the binary again under
   the selected workspace's configuration, then send `init`.

The two configurations differ only when the bootstrap prompts.
The one gap is a single-scope plugin the discovery configuration admits, where
the bootstrap then prompts and the answer selects a workspace that denies it.
It answers `describe` and nothing more: it receives no `init`, no workspace, and
no request.

### What `init` carries

`InitMessage` splits along the same seam [RFD 113] splits `Ctx`:

| Field                                                      | `single` | `multi` |
| ---------------------------------------------------------- | -------- | ------- |
| `version`, `options`, `args`, `log_level`, `output_format` | yes      | yes     |
| `paths.user_data`, `paths.user_config`                     | yes      | yes     |
| `workspace`, `paths.user_workspace`                        | yes      | absent  |
| `config`                                                   | yes      | absent  |

`config` is absent for a multi-scope plugin because there is no workspace for it
to be the configuration *of*.
The host still resolves the launch workspace's config — that is where `options`
comes from — it just does not hand over a single `AppConfig` that would be
wrong for every workspace the plugin addresses.
A multi-scope plugin reads config per checkout with `read_config`, which runs
the config pipeline for the addressed checkout, the same resolution a turn there
starts from ([RFD 113]).

### Addressing

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

The field is required for a multi-scope plugin and meaningless for a
single-scope one — decided once, by the declaration, rather than per request.
A single-scope plugin sending one, or a multi-scope plugin omitting one, is an
error rather than a silent fallback: a chat client that addresses the right
conversation ID in the wrong workspace produces no error and the wrong result.

`root` addresses one checkout of a workspace that has several.
It may be omitted only when the workspace has exactly one live checkout.
With several, omitting it is an error listing the live roots, the same way `jp
-w <id>` fails non-interactively ([RFD 087]); with none, every request is an
error.
A recency default would resolve two requests to different checkouts whenever a
`jp query` elsewhere moved `last_used` between them, and would send a
conversation continued from the web to whichever tree was touched last rather
than the one holding its edits.

There is no cross-root union.
A request names one checkout and sees that checkout's view: the user-local
durable store plus that root's projection.
Whether to merge a sibling worktree's conversations into a view is a frontend
decision, not a protocol one.

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

This is `roots::known_workspaces` projected onto the wire, with `launch` marking
the workspace `jp` was invoked against so a frontend can highlight it without
re-deriving anything.
A workspace whose every recorded checkout is gone appears with an empty `roots`
array, matching `jp w ls`.

### Where a conversation last ran

Addressing lets a frontend name a checkout; it does not tell the frontend which
one to name.
For a conversation the frontend created, it knows.
For one started with `jp query` in a terminal, only JP does.

Every turn records the checkout it runs in, in `metadata.local.json` in the
conversation's user-local directory, beside the query draft:

```json
{
  "last_root": {
    "path": "/Users/jean/Projects/jp.git/my-feature",
    "last_used": "2026-09-26T09:14:00Z"
  }
}
```

`metadata.local.json` holds a conversation's machine-local metadata: facts that
are true on this machine and meaningless on another, which is why they are not
in `metadata.json`.
`last_root` is its first key, with the shape of a roots registry entry ([RFD
087]).

- **User-local only.** Storage never copies it into a checkout's projection,
  which is committed: a persist writes only the managed files to each root, and
  importing a projected conversation copies from the checkout into user-local,
  never the other way.
  Synchronizing a checkout's projection into user-local, as a managed edit does,
  replaces the managed files and preserves every other file in the directory.
  The query draft is kept out of the checkout the same way.
- **One lifecycle with the conversation.** A title change renames the whole
  directory and removing the conversation removes it, so the file moves and goes
  with it.
- **Written when a turn first persists**, by whoever runs the turn — `jp
  query`, or the host for a delegated turn.
  Not when the lock is taken, because a conversation is stored only once a turn
  starts.
  A turn still running in a terminal has already persisted its start, so it is
  already recorded.
  The lock holder is the only writer, so reading and rewriting the file cannot
  race.
- **Forward compatible.** Every field is optional, and a rewrite preserves keys
  the writing version does not know, so an older `jp` keeps what a newer one
  recorded.
- **Liveness is derived**, with `roots::is_live` at read time, never stored.
  A record whose checkout is gone is reported as not live rather than removed:
  "last ran in a worktree that no longer exists" is what the frontend has to
  show.

Without user-local storage there is nowhere machine-local to write it, so no
record exists and the frontend asks.

`list_conversations` entries and `read_events` report it:

```json
"last_root": {
  "path": "/Users/jean/Projects/jp.git/my-feature",
  "last_used": "2026-09-26T09:14:00Z",
  "live": true
}
```

The field is absent for a conversation no turn has recorded, which includes
every conversation whose last turn predates this RFD.

A frontend's read of the record can be stale by the time it sends: a turn
finishing in a terminal moves the record after the page loaded it.
So the check happens under the conversation lock, not in the frontend.
`query` gains an optional `expected_last_root`, the record the frontend showed
the user, modeled on the `revision` precondition `write_draft` carries:

- **Inferred continuation** sends it, with `last_root.path` as the request's
  `root`.
  After taking the lock and before recording a root or starting tools, the host
  compares it with the stored record.
  On a mismatch it answers with a correlated error carrying the current record,
  and no turn starts.
- **A deliberate choice** — the user picked a checkout — omits it, and the
  turn runs where the user said.
- **A record that is not live** is shown as such ("last ran in a worktree that
  no longer exists"), and the frontend asks the user to pick one of the
  workspace's live roots, or to confirm running here.
  Confirming sends the dead record as `expected_last_root`, which matches, so
  the turn runs and records its new root.
- **An absent record** means the frontend asks.

Falling back to the launch checkout is the failure this record exists to
prevent.

The record is a report, not a binding.
Nothing in the CLI reads it: where `jp query` runs is decided by the working
directory and `--workspace`, as it is today, and a turn run elsewhere overwrites
the record.
Making it decide would mean reconciling it with the working directory on every
run, with a prompt for a divergence that is not a problem in a terminal.
The cost is that one `jp query` from another checkout moves the frontend's
default; that is the right default, because the assistant's most recent edits
are in the checkout where it last ran.

### The host's workspace registry

The host holds one `CliCtx` and a map of `WorkspaceCtx` ([RFD 113]) keyed by
checkout — a workspace ID and a root.
Two worktrees of one repository share a workspace ID, and a registry keyed by
the ID alone would run one tree's turns against the other's files.
The map is populated lazily: an addressed checkout is opened on first reference
and kept for the process lifetime.

Kept, not opened per request, because a delegated turn outlives its request.
`query` spawns a task holding a `ConversationLock` for as long as the assistant
takes, so the workspace behind it has to outlive the message that started it.

Freshness is unaffected: `list_conversations` and `read_events` already re-read
the conversation index before reading, so a `jp query` in another terminal is
visible on the next request.
That holds per checkout with no additional machinery.

MCP servers are not per checkout.
The host has one pool, on `CliCtx`, and a turn leases instances keyed by server,
configuration, and spawn directory ([RFD 113]).
A turn in another checkout gets its own instance because its spawn directory
differs, not because it has a pool of its own.
An instance starts when a turn first leases it and stops once no turn has held a
lease on it for an idle period; a checkout that is only ever read starts none.

Locks are tracked per checkout and released per checkout when the plugin exits
or dies.

Background tasks keep one handler for the process — one `JoinSet` and one
cancellation token, so Ctrl-C still stops all background work — and each task
carries its own destination: the checkout and conversation it writes to.
`TaskHandler::sync` resolves that destination through the registry instead of
receiving a single `&mut Workspace`.
Delegated turns already write their titles this way, through the turn's own
conversation lock.
A handler per checkout, the other option [RFD 113] left open, multiplies join
sets and cancellation tokens and makes Ctrl-C fan out across them, for nothing a
task that knows its destination does not already give.

### Backward compatibility

Existing plugins — `jp-path`, `jp-ticket`, `jp-serve-web` before its update —
declare nothing, resolve to `single`, and see an unchanged `init` and an
unchanged request set.
Shell-script plugins keep working with no workspace field anywhere.

Admission is unchanged except where the bootstrap prompts: then a plugin the
answered workspace denies has already answered `describe` before it is refused.

The one change a single-scope plugin sees is `last_root` on conversation
entries, an additive response field, and `expected_last_root` on `query`, an
additive request field.
A single-scope host checks the record under the lock itself: a live record for
another checkout refuses the turn, naming that checkout, and a dead one refuses
it unless `expected_last_root` confirms it.
The guarantee rests on the host, not on how recently the plugin read the record.

## Drawbacks

- **N instances of an MCP server.** A host running turns in five checkouts runs
  five instances of each server those turns use, one per spawn directory.
  The idle-shutdown policy bounds it, but the ceiling is set by how many
  checkouts a frontend touches, which the host does not control.

- **N checkouts held open.** Each carries a conversation index and open file
  handles.
  Bounded by how many the frontend addresses, and unbounded from the host's
  side.

- **A protocol version bump, and a required field.** Every multi-scope request
  carries a workspace reference, which is more verbose for a caller that
  constructs JSON by hand.
  Single-scope plugins pay nothing.

- **Two request shapes in the protocol documentation.** A reader has to know
  which scope a plugin declared to know whether the field appears.

## Alternatives

**Optional field, absent means the bootstrapped workspace.** Simpler and
strictly additive, but it creates a silent-failure class: a multi-scope plugin
that omits the field acts on the launch workspace, which for a chat client means
sending a message into the wrong workspace with no error.
The declaration makes that unrepresentable.

**Always required, with no bootstrapped path.** Breaks every existing plugin,
and more importantly pushes [RFD 087]'s targeting grammar, session state, and
conflict resolution into each one.
`jp -w foo ticket ls` would have the plugin re-resolving `foo`.
The bootstrap exists so nothing downstream re-derives it.

**Bind a conversation to a checkout.** The conversation owns a root and every
turn runs there, whatever the working directory.
In a terminal this runs tools in a tree the user is not looking at, or needs a
prompt whenever the working directory disagrees — and the disagreement only
exists because of the binding.
The last-root record gives a frontend the same default without changing where
the CLI runs.

**Record the checkout somewhere other than `metadata.local.json`.** A
per-conversation file in a separate user-local directory has a lifecycle of its
own: it has to be renamed, removed, and pruned alongside the conversation by
code that knows about both.
A root on the event stream's turn start changes the event schema, is projected
into committed checkouts, and needs older readers to tolerate it.
A map of conversations inside each roots registry entry has every turn in a
checkout rewrite one shared file.
Deriving it from session records fails for any conversation without a live
session, which includes every one started from a frontend.

**Runtime scope switching.** A `use_workspace` message binding the connection to
a workspace, with an unlock to return to explicit addressing.
Rejected on three grounds: it has no consumer, since a plugin wanting one
workspace other than the launch one is already served by `jp -w foo <plugin>`;
it reintroduces connection-global state into a protocol that has correlation IDs
precisely so several requests can be outstanding, so a switch has undefined
meaning for in-flight requests; and `use` already names the session-scoped
selection ([RFD 087]), which is a mechanically different concept.
A plugin with this need can declare `multi` and address explicitly.

**Let the plugin open workspaces itself**, by linking `jp_workspace` or calling
through `jp_ffi`.
Available only to Rust and C callers, and insufficient regardless: a plugin
cannot run a turn without the user's credentials, the tool registry, and the MCP
servers, which is why the host owns the agent loop.

**A subordinate `jp` process per workspace, proxied by the host.** Keeps each
host single-workspace and buys failure isolation, at the cost of putting JP's
bootstrap semantics — targeting, session, conflict prompts — outside the host
that owns them.
It also pays a process per workspace to avoid holding a struct per workspace.

**Wait for [RFD 027].** A client-server architecture subsumes this, and is much
larger and unbuilt.
Growing the working protocol is the cheaper path, and 027 inherits the
addressing if it lands.

## Non-Goals

- **Authentication and authorization.** A plugin sees whatever the host can
  reach, and this RFD widens that to every workspace on the machine while the
  web frontend binds an unauthenticated port.
  That is accepted for internal use.
  Access grants for plugins are separate work, taken up when the frontend is
  solid enough to warrant them.

- **Cross-root conversation views.** A request addresses one checkout.
  Merging sibling worktrees' conversations is a frontend concern.

- **Runtime scope switching.** See Alternatives.
  Additive later if a consumer appears.

- **New capabilities.** Every request this RFD makes addressable already exists.
  Nothing new is added to the protocol beyond `list_workspaces`, the reference
  field, the `last_root` report, and the `expected_last_root` precondition.

- **The application-layer extraction.** [RFD 113] records where each capability
  belongs; moving them into their own crate is later work.

## Risks and Open Questions

- **MCP resource growth is bounded only by frontend behavior.** A frontend that
  delegates turns across many checkouts starts many server instances.
  The idle policy needs a concrete timeout and a way for the user to cap it.

- **Protocol version numbering.** The version counter documents an eight-step
  evolution no external consumer lived through.
  If the protocol crate is reshaped alongside this work, version 1 should
  describe what the protocol *is* rather than how it got here.

- **`launch` on a listing entry.** It marks the workspace `jp` was invoked
  against, which a multi-scope plugin was told nothing else about.
  Whether that is a useful hint for highlighting a default or an inconsistency
  with "told nothing about the launch workspace" is worth a second look.

- **Help and admission.** `jp <plugin> --help` and root help follow [RFD 072]'s
  rule that admission precedes every spawn, `describe` included.
  Until that lands, bare help answers from `describe` without the deny or
  approval checks.

## Implementation Plan

### Phase 1: The last-root record

- Introduce `metadata.local.json` in the conversation's user-local directory,
  preserving unknown keys on rewrite.
- Make projection sync replace the managed files instead of the directory, so
  machine-local files survive a managed edit.
- Write `last_root` when a turn from `jp query` or a delegated turn first
  persists.
- Report `last_root` on `list_conversations` entries and `read_events`.
- Add `expected_last_root` to `query`, and check the record under the
  conversation lock: a live record for another checkout refuses the turn; a dead
  one refuses it unless confirmed.
- Have `jp-serve-web` show a dead record and send the confirmation.

Does not depend on [RFD 113].
Mergeable alone: a web turn aimed at the wrong tree becomes an error instead of
an edit in the wrong tree.

### Phase 2: Scope declaration

- Add `workspace_scope` to `DescribeResponse`, defaulting to `single`.
- Derive the plugin dispatch's `WorkspaceRequirement` from it.
- Admit under the discovery configuration before `describe`, and again under the
  selected workspace's configuration for a single-scope plugin.
- Make `init`'s workspace and config fields conditional on the declared scope.
- Reject a plugin whose declaration and requests disagree.

Depends on [RFD 113] Phase 1.
Mergeable alone: with no plugin declaring `multi`, behavior is unchanged except
where the bootstrap prompts, in which case a plugin the answered workspace
denies has already answered `describe`.

### Phase 3: `list_workspaces`

- Project `roots::known_workspaces` onto the wire, with per-root storage paths
  and the `launch` marker.

Depends on Phase 2.
Mergeable alone, and enough for a frontend to show a workspace list before it
can open one.

### Phase 4: Addressing and the registry

- Add `WorkspaceRef` to the data and mutation requests, requiring `root` when a
  workspace has more than one live checkout.
- Populate `WorkspaceCtx` lazily per addressed checkout; track and release locks
  per checkout.
- Carry each background task's destination and resolve it through the registry.
- Implement the idle-shutdown policy for unleased MCP server instances.
- Update `jp-serve-web` to declare `multi`, address explicitly, and continue a
  conversation in its `last_root` with `expected_last_root` set, asking for a
  checkout when there is none; a checkout the user picks is sent without it.

Depends on Phases 1 and 3.

## References

- [RFD 113] — Context decomposition; provides `CliCtx` / `WorkspaceCtx` and the
  invocation-scoped configuration this RFD's `multi` declaration is admitted
  from.
- [RFD 072] — Command plugin system; the protocol this RFD extends.
- [RFD 087] — Session-scoped active workspace; the roots registry,
  `known_workspaces`, the targeting grammar that selects the launch workspace,
  and the conventions the last-root record follows.
- [RFD 031] — Durable conversation storage; defines the projection model and
  `StoragePresence`, which is why a checkout is addressable at all.
- [RFD 073] — Layered storage backend; the load path each addressed workspace
  reads through.
- [RFD 099] — Native macOS app; the frontend whose window-per-workspace model
  wants the same capability through a different adapter.

[RFD 027]: 027-client-server-query-architecture.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 072]: 072-command-plugin-system.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 099]: 099-native-macos-app-for-browsing-conversations.md
[RFD 113]: 113-context-decomposition-for-invocation-and-workspace-scope.md
