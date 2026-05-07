# RFD 080: Editor as a Config Source

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-04
- **Requires**: [RFD 079](079-config-sources-and-load-order.md)

## Summary

Move the query editor invocation out of `Query::run` and into the startup
pipeline (`run_inner`). The editor's TOML preamble becomes another input to
config resolution: `resolve_config` returns a `PartialAppConfig`; the editor is
given that partial (with defaults applied); the editor's resulting delta is
layered on top, and the final `AppConfig` is built. The `Ctx::config`
immutability rule is preserved.

## Motivation

In `Query::run`, `ctx.config()` is captured once. The editor opens later in the
same function via `build_conversation` → `edit_message` → `editor::edit_query`.
The editor's `PartialAppConfig` output is recorded as a `config_delta` event on
the conversation, but every subsequent read in the same turn
(`cfg.conversation.tools`, `cfg.conversation.attachments`, `cfg.assistant.*` in
`handle_turn`) uses the *pre-editor* `cfg`. The recorded delta is folded into
the resolved config only on the *next* invocation, when
`Query::apply_conversation_config` reads the events stream.

The editor is a config source; treating it as anything else produces tech debt.
This RFD folds it through the same pipeline as every other source.
Lighter-weight patches that don't restructure the pipeline are discussed in
[Alternatives](#alternatives) and rejected.

## Design

### Flow

```txt
parse CLI → load workspace → workspace.load_conversation_index()
  → resolve session → build_runtime → SignalPair::new(&runtime)
  → resolve_partial: source loading happens here, exactly once.
     Returns a PartialAppConfig with all sources merged
     (base + per-conv events + --cfg + CLI flags).
  → if command implements EditableCommand:
       extract the query's ConversationHandle from `handles` (move-only)
       pre_editor_cfg = build(partial.clone())   // validates + resolves aliases
       acquire lock (rt.block_on):
         --new path: pre_editor_cfg seeds the new conversation's base config
         existing path: lock the resolved handle directly
       invocation_delta = stream.config()?.to_partial().delta(pre_editor_cfg.to_partial())
       record invocation_delta now (so a subsequent editor failure still
         persists CLI intent, matching today's MissingEditor behaviour)
       run_editor_protocol(cmd, pre_editor_cfg.to_partial(), lock) → EditorOutcome
       on Run { editor_delta, output }:
         candidate = load_partial(partial.clone(), editor_delta)
         candidate_cfg = build(candidate)        // validate before persisting
         editor_delta.resolve_model_aliases(&candidate_cfg.providers.llm.aliases)
         record editor_delta if non-empty
         cfg = candidate_cfg                     // candidate reused as final
       on Abort (pre-open; Query never emits this today):
         cfg = pre_editor_cfg                    // pre_editor reused as final
  → for non-editable: cfg = build(partial)
  → Ctx::new (immutable thereafter)
  → command.run (Query handles Option<PreparedQuery>; empty case is internal)
```

### Precedence

```
implicit base (files + env)
  < per-conv events (existing config_delta events)
  < --cfg
  < CLI flags
  < editor delta            ← top (current turn)
```

The editor sits at the top of the stack for the current turn — that's where
the user just expressed their intent. After the turn, the same delta is
recorded as a `config_delta` event in the conversation. On a subsequent run,
`apply_conversation_config` folds it into the per-conversation layer along
with all other events; the new invocation's `--cfg` and CLI flags override
it. The rule "the user's most recent action wins" applies in both cases —
the editor delta's relative precedence shifts only because the *user's most
recent action* shifts (current turn: the editor; next turn: the new command
line).

### Resolution split

Today's `resolve_config` in `jp_cli` ends with `build(partial)`. The split:

- `resolve_partial` — same body, returns `(PartialAppConfig,
  Vec<ConversationHandle>)`. The `partial.conversation.default_id.take()` line
  stays here. **Source loading (files + env + `--cfg` files) happens here,
  exactly once per invocation.**
- `build(partial)` — moves into `run_inner`. `build` is a pure schematic
  validation + defaults pass with no I/O.

Build count per invocation:

- non-editable command: 1 `build` (the final cfg).
- editable command, `EditorOutcome::Run`: 2 `build`s — `pre_editor_cfg`
  (validates partial, resolves aliases) and `candidate_cfg` (validates partial +
  editor delta). The candidate becomes the final cfg, reused by `Ctx::new`.
- editable command, `EditorOutcome::Abort` (pre-open abort, currently unused
  by Query): 1 `build` — `pre_editor_cfg`, reused as the final cfg.

Source loading does not repeat across multiple `build` calls.

The editor flow consumes the partial directly:

- `editor::edit_query` and `Query::build_conversation` take `&PartialAppConfig`
  instead of `&AppConfig`. The `[config]` preamble already constructs a partial
  internally via `to_partial()` (see `build_config_text`); with a partial input
  it reads the relevant fields directly.
- New method `PartialEditorConfig::command()` mirrors `EditorConfig::command()`,
  reading `self.cmd: Option<String>` and `self.envs: Option<Vec<String>>`
  directly. The dispatch site passes `pre_editor_cfg.to_partial()` (already
  defaults-filled by `build`) so `PartialEditorConfig::command()` sees the
  resolved `envs`.

For non-editable commands the flow is `resolve_partial` → `build` → `Ctx::new`,
behaviourally identical to today.

### Computing the editor delta

The editor preamble is *seeded* with values from the resolved partial. An
unchanged preamble reproduces those values, so recording the parsed preamble
as-is would persist seeded fields as if the user had typed them (e.g., a
`--model` CLI flag would re-appear as an editor-authored `config_delta` event).

The extraction:

```txt
editor_delta = seed_partial.delta(parsed_partial)
```

`PartialConfigDelta::delta` is the existing primitive used elsewhere for this
kind of diff. Only `editor_delta` is folded into the partial and persisted.
Today's `editor::edit_query` skips this step and exhibits a phantom-delta bug;
the extraction fixes it as part of moving editor invocation pre-dispatch.

**Limitations.** `seed.delta(parsed)` is additions-only:

- Deleting a seeded scalar field from the preamble produces "no delta"
  (per `delta_opt` semantics), not an unset directive.
- List-like fields (e.g., `conversation.attachments`,
  `conversation.tools`) are additions-only too: reordering or removing
  seeded list items does not persist as a delta. The editor preamble is
  not a lossless config editor for collections.

Real removals and reorderings require RFD 070's unset semantics.

### The `EditableCommand` trait

Commands that drive `$EDITOR` implement a sibling trait to
`IntoPartialAppConfig`:

```rust
pub(crate) trait EditableCommand: IntoPartialAppConfig {
    /// Command-specific payload extracted from the run.
    type EditorOutput: Send;

    /// Decide whether the editor should open, and what to seed it with.
    /// May also return a payload directly when bypassing the editor (e.g.,
    /// query passed as argv with --no-edit), or signal abort up-front.
    fn editor_input(
        &self,
        partial: &PartialAppConfig,
        lock: &ConversationLock,
        // also: workspace, fs_backend, stdin handle
    ) -> Result<EditorRequest<Self::EditorOutput>, BoxedError>;

    /// Parse editor output. May request a retry (e.g., re-render the
    /// preamble with an inline parse error) or report a post-edit
    /// abort (empty edited content). Retries are resolved by the
    /// protocol's loop; the dispatch site sees only Run/Abort.
    fn parse_editor_output(
        &self,
        partial: &PartialAppConfig,
        raw: &str,
    ) -> Result<ParseOutcome<Self::EditorOutput>, BoxedError>;
}

pub(crate) enum EditorRequest<O> {
    /// Open the editor with this seed; afterwards, call `parse_editor_output`.
    Open(EditorInput),
    /// Skip the editor; here's the payload directly (no config delta).
    Skip(O),
    /// Don't run the command up-front (e.g., precondition not met).
    Abort,
}

/// What `parse_editor_output` returns. `Retry` is consumed by the protocol
/// loop and never propagates to the dispatch site.
///
/// There is no `Abort` variant: post-edit empty queries (e.g., user
/// opened the editor but left the body blank) are encoded as `Run`
/// with an `output` whose command-specific shape signals "empty"
/// (for `Query`, `PreparedQuery::request = None`). This preserves
/// `query_file` and other editor state for cleanup.
pub(crate) enum ParseOutcome<O> {
    /// Editor produced a (possibly empty) delta and a usable payload.
    Run { delta: PartialAppConfig, output: O },
    /// Reopen the editor with this input (e.g., parse error annotated
    /// back into the preamble). The protocol loop drives the editor
    /// again and re-invokes `parse_editor_output`.
    Retry(EditorInput),
}

/// What `run_editor_protocol` returns. `Retry` is resolved internally.
pub(crate) enum EditorOutcome<O> {
    Run { delta: PartialAppConfig, output: O },
    Abort,
}
```

Today only `Query` implements `EditableCommand`, with `EditorOutput =
PreparedQuery` (a struct wrapping `ChatRequest` plus editor-flow state — see
"Trimmed `Query::run`" below). The trait isn't there to share code across
consumers — `editor_input` and `parse_editor_output` are entirely bespoke per
command. It keeps the editor protocol contract free of command-specific types:
`run_editor_protocol` is generic, and the trait method signatures mention only
`PartialAppConfig` and `Self::EditorOutput`. The `Commands::Query(q)` match in
`run_inner` itself isn't hidden — that's the natural shape of enum-based command
dispatch — but query domain types (`ChatRequest`, the `[config]/[history]`
document shape) stay inside `Query`'s module.

The generic helper:

```rust
async fn run_editor_protocol<C: EditableCommand>(
    cmd: &C,
    pre_editor: &PartialAppConfig,
    lock: &ConversationLock,
) -> Result<EditorOutcome<C::EditorOutput>, BoxedError> {
    let mut input = match cmd.editor_input(pre_editor, lock)? {
        EditorRequest::Abort => return Ok(EditorOutcome::Abort),
        EditorRequest::Skip(output) => return Ok(EditorOutcome::Run {
            delta: PartialAppConfig::empty(),
            output,
        }),
        EditorRequest::Open(input) => input,
    };
    loop {
        let raw = drive_editor(input)?;
        match cmd.parse_editor_output(pre_editor, &raw)? {
            ParseOutcome::Run { delta, output } => {
                return Ok(EditorOutcome::Run { delta, output });
            }
            ParseOutcome::Retry(next) => input = next,
        }
    }
}
```

The retry loop preserves today's behaviour: invalid TOML in the `[config]` block
re-renders the preamble with an inline error message and reopens the editor with
the user's edits intact.

Dispatch in `run_inner`:

```rust
// Query consumes its handle (move-only); the lock replaces it for Query::run.
let mut handles = handles;
let query_handle = matches!(&cli.command, Commands::Query(_))
    .then(|| handles.pop()).flatten();

let (lock_for_query, prepared, cfg) = if let Commands::Query(q) = &cli.command {
    // Build pre-editor cfg early: validates the partial and resolves aliases.
    let pre_editor_cfg = build(partial.clone())?;
    let pre_editor_partial = pre_editor_cfg.to_partial();

    let lock = acquire_lock_for_query(
        q, query_handle, &pre_editor_cfg, &workspace, &session, &signals,
    ).await?;

    // What this invocation adds on top of the persisted stream
    // (--cfg + CLI flags), with aliases already resolved by the build.
    let stream_partial = lock.events().config()?.to_partial();
    let invocation_delta = stream_partial.delta(pre_editor_partial.clone());

    // Record invocation_delta now — if the editor fails afterwards
    // (MissingEditor, IO error), this still reflects CLI intent
    // (matches today's behaviour where get_config_delta_from_cli
    // recorded before the empty-query check).
    if !invocation_delta.is_empty() {
        lock.as_mut().update_events(|e| e.add_config_delta(invocation_delta));
    }

    match run_editor_protocol(q, &pre_editor_partial, &lock).await? {
        EditorOutcome::Run { delta: mut editor_delta, output } => {
            // Validate the merged partial. A failed build leaves the
            // editor delta unrecorded (`invocation_delta` already
            // persisted above represents only what the CLI specified).
            let candidate = load_partial(partial.clone(), editor_delta.clone())?;
            let candidate_cfg = build(candidate.clone())?;

            // Resolve aliases against the post-merge alias map (handles
            // aliases defined and used in the same preamble).
            editor_delta.resolve_model_aliases(&candidate_cfg.providers.llm.aliases);

            if !editor_delta.is_empty() {
                lock.as_mut().update_events(|e| e.add_config_delta(editor_delta));
            }
            // candidate_cfg is the final cfg — no extra build needed.
            (Some(lock), Some(output), candidate_cfg)
        }
        EditorOutcome::Abort => {
            // Pre-open abort. Query never returns EditorRequest::Abort, so
            // this branch is unused for Query today. Reserved for future
            // commands. pre_editor_cfg is the final cfg — no extra build.
            (Some(lock), None, pre_editor_cfg)
        }
    }
} else {
    (None, None, build(partial)?)
};

let mut ctx = Ctx::new(..., cfg, signals, ...);
let result = rt.block_on(cli.command.run(&mut ctx, handles, lock_for_query, prepared));
// Post-Ctx finalization runs unconditionally. Pre-Ctx `?` errors above
// route through reduced finalization — see "Finalization" below.
```

For `Query`, `command.run` always invokes `Query::run` — the empty/abort case is
no longer special-cased at dispatch. `Query::run` handles `prepared = None`
internally (see "Trimmed `Query::run`").

`drive_editor` is the existing `editor::open` plumbing, taking a structured
`EditorInput`:

```rust
struct EditorInput {
    path: Utf8PathBuf,           // QUERY_MESSAGE.md location
    seed: QueryDocument,         // initial preamble + body rendered to the file
    parse_error: Option<String>, // inline annotation rendered on retry
}
```

The existing `RevertFileGuard` semantics carry through: `drive_editor` creates
the file (or reuses an existing one) with the seed content, the guard restores
original content if not disarmed, successful parsing disarms before returning.
On `ParseOutcome::Retry`, the protocol must preserve the edited file contents
for the next editor invocation — either by disarming the guard before re-driving
the editor, or by carrying the edited raw content into the next `EditorInput`.
Reusing the same `path` alone is not sufficient if the guard restores between
invocations.

The body of today's `editor::edit_query` splits cleanly: the `QueryDocument`
construction and seed-content logic moves into `Query::editor_input`; the
`QueryDocument::try_from(content)` reparse, seed-vs-parsed delta extraction (see
"Computing the editor delta" above), and TOML parse-error retry logic move into
`Query::parse_editor_output`.

The early-return paths in today's `Query::edit_message` (no-edit + replay,
query-as-argv, missing editor) translate to `EditorRequest::Skip` (with the
chat_request built from argv/stdin/replay) or propagate as errors (e.g.,
`MissingEditor` when no editor is configured and no chat_request is
available). `EditorRequest::Abort` is reserved for hypothetical future
commands and is not produced by `Query` today.

The associated-type non-object-safety is intentional. Dispatch matches on the
concrete enum variant, so static dispatch is the natural shape; we don't need
`Box<dyn EditableCommand>`.

### Pre-dispatch lock acquisition

The editor needs a stable conversation root to write `QUERY_MESSAGE.md` and the
events stream for the history preamble — both come from the lock. Acquiring the
lock pre-`Ctx` requires:

- `LockRequest` already takes individual borrows (`workspace`, `handle`,
  `is_tty`, `session`, `printer`, `signals`, `lock_wait`). All other fields are
  available pre-`Ctx`; `signals` is currently constructed inside `Ctx::new` and
  must move out — `SignalPair::new(&runtime)` becomes a step in `run_inner`
  between `build_runtime` and lock acquisition, then is handed to both
  `LockRequest` and `Ctx::new`.
- `acquire_lock` is async, so we need the runtime. `build_runtime` runs before
  `Ctx::new` today, so we can `rt.block_on(...)` here.
- `Query::acquire_lock` calls `create_new_conversation(ctx)` and
  `fork_conversation(ctx, ...)`. Both take `&mut Ctx` today. They need
  refactoring to take individual deps:
  - `create_new_conversation` reads `ctx.config()`, `ctx.workspace`,
    `ctx.session` — straightforward to pass directly.
  - `fork_conversation` (the shared helper in `conversation/fork`) reads
    `ctx.now()`, `ctx.workspace`, `ctx.session` — straightforward to pass
    directly.
- The `--new` path requires a built `AppConfig`: it seeds the new conversation's
  base config via `Workspace::create_and_lock_conversation`. We satisfy this by
  calling `build(partial.clone())` once for new-conversation creation. The clone
  is pure in-memory work and `build` is pure validation/defaults; source loading
  does not repeat.

### Trimmed `Query::run`

`Query::run` receives the lock and `Option<PreparedQuery>`:

```rust
struct PreparedQuery {
    request: Option<ChatRequest>,
    query_file: Option<Utf8PathBuf>,
    opened_editor: bool,
}
```

- `prepared = Some(pq)` is the normal Query path. `pq.request: Some(_)` is a
  usable chat request; `pq.request: None` is a post-edit empty query (editor
  opened, body left blank, possibly with config edits in the preamble).
  `query_file` is `QUERY_MESSAGE.md`'s path for echoing and cleanup;
  `opened_editor` distinguishes editor-produced requests from CLI/stdin-only
  ones.
- `prepared = None` covers the pre-open `EditorRequest::Abort` path
  (precondition not met before opening the editor). Query's `editor_input`
  doesn't currently return `Abort`, so this branch is unused in practice but
  kept for trait completeness; if reached, `Query::run` returns `Ok(())`
  immediately without running any side effects.

For `prepared = Some(pq)`, `Query::run` always runs:

- `--title` metadata update
- session activation
- pre-query compaction (when `--compact` is set)

Additional steps when `pq.request.is_some()`:

- `configure_active_mcp_servers`
- title task spawn
- tool definitions / attachment loading
- thread building, `handle_turn`

When `pq.request.is_none()`: print "Query is empty, ignoring." and clean up
`pq.query_file`. MCP servers do not boot for an empty edited query — a small
intentional behaviour change from today (see Risks).

`query_file` cleanup matches today's behaviour: removed on a successful turn,
removed on post-edit empty query, **preserved on turn error** so the user
can recover their composed text from `QUERY_MESSAGE.md`.

The editor invocation, `acquire_lock`, alias resolution, editor-delta recording,
and the invocation-delta computation (today's `get_config_delta_from_cli`) all
leave `Query::run` for `run_inner`. The steps that stay read from `ctx.config()`
(the post-editor cfg), so editor-provided values take effect.

### Finalization for pre-`Ctx` errors

Today's `run_inner` finalization runs after `command.run` and uses `ctx`:
printer flush, `task_handler.sync(...)`, `remove_ephemeral_conversations`,
`cleanup_stale_files`. Under this RFD, errors can occur pre-`Ctx`
(`build(partial)?`, lock acquisition, editor failure). They cannot run the
post-`Ctx` block as-is.

Reduced pre-`Ctx` finalization runs printer flush + workspace cleanup (both
available from locals already in scope) and skips task-handler sync. The skip is
intentional: no tasks have been spawned at that point, so sync would be a no-op
anyway. The `task_handler` itself is owned by `Ctx`, so calling sync without
`Ctx` isn't structurally possible.

Implementation: extract today's finalization block into `post_ctx_finalize(&mut
ctx)`; add `pre_ctx_finalize(&mut workspace, &printer, fs_backend)`. Pre-`Ctx`
`?` errors in `run_inner` route through the latter (e.g., via a small RAII guard
or explicit `map_err` + cleanup); post-`Ctx` paths use the former.

### MCP server activation timing

Today `configure_active_mcp_servers` runs *before* the editor and starts MCP
boot in parallel with user typing. After this RFD it runs *after* the editor, so
we lose that overlap (typically 1–2s of added latency for long edit sessions).

A correctness improvement falls out: editor changes to `[providers.mcp.*]` and
`[conversation.tools.*]` *do* take effect for the current turn, since
`mcp_client` and the active-tool list are derived from the post-editor cfg.
Today silently drops these; the RFD fixes it.

## Non-Goals

- Relaxing `Ctx::config` immutability. Preserving it is the point.
- Changing the `config_delta` event schema or how deltas are stored.
- Generalizing "mid-run config sources" beyond the editor.
- Restoring the parallel MCP-startup-with-editor optimization. If it matters, a
  follow-up can build `mcp_client` pre-`Ctx` from the partial and pass it into
  `Ctx::new`.

## Alternatives

Two patches that don't restructure the resolution pipeline were considered and
rejected:

- **Local rebind** of `cfg` in `Query::run` after the editor — fixes the visible
  failure but leaves `ctx.config()` returning the stale snapshot. Future
  `ctx.config()` callers in the post-editor path silently regress.
- **`Ctx::refine_config` setter** — qualifies the documented "immutable
  post-init" rule, and would leave `mcp_client` (built once in `Ctx::new` from
  `config.providers.mcp`) tied to the pre-editor cfg, recreating the same
  split-source-of-truth problem in a different field.

Both leave the editor as a special case rather than a config source. The
pre-dispatch flow this RFD proposes treats it uniformly, at the cost of
restructuring `run_inner` and the lock acquisition path.

## Risks

- **`fork_conversation` / `create_new_conversation` refactor.** Both currently
  couple lock acquisition to `&mut Ctx`. Moving them pre-`Ctx` changes their
  call sites in `query` and the conversation subcommands (`archive`, `compact`,
  `edit`, `rm`). Most of those don't fork or create — they only need
  `LockRequest::from_ctx`, which keeps working.
- **Trait surface stability.** `EditableCommand` is designed against one
  consumer; its shape may shift when a second implementor lands. The
  `EditorRequest` enum keeps the dispatch-layer commitment minimal.
- **`Commands::run` signature for Query.** Query's `run` needs to receive
  `(lock, PreparedQuery)` from the dispatch layer; other variants don't. The
  `Commands::run` match already dispatches per-variant, so this is a localized
  signature change in the Query arm, not a trait generalization.
- **MCP startup timing.** Today MCP boot runs in parallel with the editor (saves
  1–2s on long edit sessions). Under this RFD it runs after the editor, adding
  that latency. Additionally, MCP startup is skipped entirely for empty queries
  — today boots MCP even when no query is sent. Both are acceptable trade-offs
  for first cut; revisit if measured.
- **Error-reporting paths.** Editor errors used to flow through `cmd::Error`
  from `Query::run`; they now abort in `run_inner`. Printer and error machinery
  are available pre-`Ctx`, so user-visible output should be equivalent. Verify
  with `MissingEditor`.
- **Template rendering.** Stays in the pre-dispatch flow against the partial's
  `template.values`. Editor-supplied template values do not affect the current
  turn (same as today's behaviour). Template rendering must happen on both
  the `Skip` and `Run` paths (see Implementation Plan step 6); today the
  same `if self.template` block runs unconditionally after `edit_message`.
- **Pre-editor failure side effects.** Today, `MissingEditor` (and any
  other failure during `edit_message`) occurs inside `Query::run` after
  session activation, `--title` metadata update, `--compact` compaction,
  and `configure_active_mcp_servers`. Under this RFD, `MissingEditor`
  and similar pre-`Ctx` failures abort before `Query::run`, so those
  side effects don't run. The change is benign for `MissingEditor` (no
  turn happens; activating session/title/MCP for a failed editor lookup
  is wasted work) but is a behaviour change worth noting.

## Implementation Plan

1. Fix `jp_config::delta::delta_opt_vec` for Vec fields that rely on it:
   it currently returns `None` when every element of `prev` is contained
   in `next`, even if `next` has additional elements — silently dropping
   additions (e.g., `prev.len() == next.len()` check). Note that
   `conversation.attachments` uses separate additions-only logic in
   `PartialConversationConfig::delta` and is unaffected by
   `delta_opt_vec`, but should be covered by the same seed-vs-parsed
   tests since editor-added attachments are the motivating case for
   this RFD.
2. Split `resolve_config` into `resolve_partial` (returns `(PartialAppConfig,
   Vec<ConversationHandle>)`) and a `build(partial)` call in `run_inner`.
3. Add `PartialEditorConfig::command()` mirroring `EditorConfig::command()`;
   unit-test parity.
4. Move `SignalPair::new(&runtime)` from `Ctx::new` into `run_inner`; pass into
   both pre-dispatch helpers and `Ctx::new`.
5. Refactor `fork_conversation` and `create_new_conversation` to take individual
   deps instead of `&mut Ctx`. Existing fork tests should still pass.
6. Define `EditableCommand`, `EditorRequest`, `ParseOutcome`, and
   `EditorOutcome`. Implement for `Query`: `editor_input` relocates
   `build_conversation`'s stdin/argv/replay/seed-build logic and returns
   `EditorRequest<PreparedQuery>` (`Skip` for the query-as-argv / `--no-edit`
   paths — with template rendering applied to the final output before
   returning, since `Skip` bypasses `parse_editor_output`; `Abort` is
   reserved and unused by `Query`); `parse_editor_output` parses the saved
   preamble, computes `editor_delta = seed.delta(parsed)`, runs template
   rendering on the edited content, and returns `ParseOutcome::Run { delta,
   output }` or `ParseOutcome::Retry(EditorInput)` for TOML parse errors
   with inline error annotation. Empty edited queries are encoded as `Run`
   with `output.request: None` (preserving `query_file` for cleanup).
   Behaviour-preserving relocation. Both paths must render templates before
   returning so that today's unconditional `if self.template { ... }` block
   in `build_conversation` is preserved.
7. Add `run_editor_protocol` (with internal retry loop on
   `ParseOutcome::Retry`). In `run_inner`, after `resolve_partial` and signal
   init: extract the query's `ConversationHandle` from `handles` (move-only).
   For editable commands: build `pre_editor_cfg` early, acquire the lock (using
   `pre_editor_cfg` for `--new`'s seed), compute the invocation delta from
   `pre_editor_cfg.to_partial()`, **record `invocation_delta` immediately** (so
   a subsequent editor failure still persists CLI intent). Run the editor
   protocol. On `EditorOutcome::Run`: build a candidate, resolve editor-delta
   aliases against the candidate's alias map, record `editor_delta` if
   non-empty; the candidate becomes the final cfg. On `EditorOutcome::Abort`:
   `pre_editor_cfg` becomes the final cfg (no rebuild). Then `Ctx::new` and
   `command.run` (always invoked — `Query::run` handles the empty case
   internally). Pre-`Ctx` errors route through `pre_ctx_finalize`.
8. Change `Query::run` to accept `(lock, Option<PreparedQuery>)`; remove the
   editor invocation, `acquire_lock`, alias resolution, editor-delta recording,
   and `get_config_delta_from_cli` (all now in `run_inner`). On `prepared =
   None` (pre-open abort, unused for Query): return `Ok(())` immediately. On
   `Some(pq)` with `pq.request.is_none()` (post-edit empty): run `--title`,
   session activation, `--compact`; print "Query is empty, ignoring."; clean up
   `pq.query_file`; return. On `Some(pq)` with `pq.request.is_some()`: full
   turn (current behaviour with `query_file` removed only on success,
   preserved on error).
9. Verify end-to-end:
   - All existing query paths (`jp q`, `jp q "msg"`, `jp q --new`, `jp q
     --fork`, `jp q --replay`, `jp q --no-edit`) keep current behaviour, except
     for the documented MCP-startup-skip on empty edited queries.
   - Editor-added `conversation.attachments` are visible to the same query turn
     (the regression motivating this RFD).
   - CLI `--attachment` plus editor-added attachment preserves intended order on
     the *next* invocation (validates persisted event order: invocation delta
     before editor delta).
   - `jp q --new` with an editor-provided model uses that model the same turn
     (covers the `--new` two-build path).
   - Unchanged editor preamble after `--model X` produces no phantom
     `config_delta` event (covers seed-vs-parsed extraction).
   - `jp q --model alias` persists a concrete model ID (resolved alias) in the
     invocation delta event, not the alias name (covers invocation-delta alias
     resolution).
   - Semantically-invalid editor config (e.g., unknown provider, unresolved
     alias) leaves the editor delta unrecorded. If `invocation_delta` was
     non-empty, only `invocation_delta` persists — the validate-before-persist
     rule applies to the editor delta, not the (already-recorded) invocation
     delta.
   - Editor preamble that defines and uses a new model alias resolves correctly
     for the current turn and persists with the alias resolved.
   - Deleting a seeded field from the editor preamble is a no-op.
   - `jp q --template "..." "message"` (the `Skip` path, no editor opens)
     still renders templates against `template.values` and rejects
     undefined variables — same as today's `build_conversation` behaviour.
   - Post-edit empty query (`PreparedQuery` with `request: None`) cleans up
     `QUERY_MESSAGE.md`, releases the lock, and runs the standard `run_inner`
     finalization (printer flush, task sync, ephemeral cleanup).
   - Turn error after a non-empty editor-produced query *preserves*
     `QUERY_MESSAGE.md` so the user can recover their composed text
     (matches today's behaviour).
   - Invalid TOML in the editor's `[config]` block reopens the editor with the
     user's edited content preserved AND the parse error annotated inline
     (covers `ParseOutcome::Retry` plus the `RevertFileGuard` lifecycle on
     retry).
   - Pre-`Ctx` error (e.g., `MissingEditor`) runs `pre_ctx_finalize`: printer
     flushes, workspace stale-file cleanup runs.
   - Missing editor renders the same user-facing error as today, but pre-`Ctx`.

Phases 1–6 are independently mergeable refactors. Phases 7–8 land the
behaviour change. Phase 9 is verification.

## References

- RFD 054 — `config_delta` event semantics reused here.
- RFD 070 — source-tagged claims. If 070 lands first: the editor delta is
  emitted as `ApplyDelta` with source `editor:query`; the invocation delta uses
  `cli:flag` / `cli:cfg` per directive; for `--new`, the tentative `AppConfig`
  build is validation-only — conversation creation uses RFD 070's `base + init`
  partition with these deltas as init entries. If this RFD lands first: events
  have no claims, and 070 must treat them as legacy.
- RFD 079 — source/precedence model this RFD extends.
- Issue #217 — original `QUERY_MESSAGE.md` config-surface proposal.
- Issue #91 — broader query-editor protocol concerns.
