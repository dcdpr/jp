# RFD 080: Editor as a Config Source

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-04
- **Requires**: [RFD 079]
- **Summary**: Moves query editor invocation into the startup config pipeline so
  editor TOML changes apply to the current turn.

## Summary

This RFD moves query editor invocation out of `Query::run` and into the startup
configuration pipeline (`run_inner`).
`resolve_partial` produces the pre-editor `PartialAppConfig`, the editor runs
against that partial, and the resulting delta is layered on top before building
the final immutable `AppConfig` for the turn.

## Motivation

`Query::run` captures `ctx.config()` at the start of the function, before
opening the editor via `build_conversation` → `edit_message` →
`editor::edit_query`.
The editor's `PartialAppConfig` output is recorded as a `config_delta` event on
the conversation, but every subsequent read in the same turn
(`cfg.conversation.tools`, `cfg.conversation.attachments`, and `cfg.assistant.*`
in `handle_turn`) still uses the pre-editor `cfg`.
The recorded delta takes effect only on the next invocation, when
`Query::apply_conversation_config` replays the event stream.

The editor is a configuration source and belongs in the same startup pipeline as
every other source.
[Alternatives](#alternatives) covers smaller patches that leave the pipeline
unchanged and why they fall short.

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

The editor sits at the top of the stack for the current turn, where it is the
user's most recent action.
The delta is also recorded as a `config_delta` event on the conversation.
On subsequent invocations, `apply_conversation_config` folds that event into the
per-conversation layer, where new `--cfg` and CLI flags override it.
In both cases, the user's most recent action wins: the editor on the current
turn, and the command line on the next turn.

### Splitting config resolution

`resolve_config` in `jp_cli` currently loads all sources and ends with
`build(partial)`.
This RFD splits that work into two steps:

- `resolve_partial` keeps the existing body (including
  `partial.conversation.default_id.take()`) and returns `(PartialAppConfig,
  Vec<ConversationHandle>)`.
  All source loading (files, environment variables, and `--cfg` files) happens
  here, once per invocation.
- `build(partial)` moves into `run_inner`.
  `build` is a pure schematic validation and defaults pass with no I/O, so
  calling it more than once does not reload sources.

How many times `build` runs depends on the command path:

- **Non-editable commands** call `build` once (`resolve_partial` → `build` →
  `Ctx::new`), which is behaviorally identical to today.
- **Editable commands (`EditorOutcome::Run`)** call `build` twice: first for
  `pre_editor_cfg` (validating the partial and resolving aliases) and then for
  `candidate_cfg` (validating the partial merged with the editor delta).
  `Ctx::new` reuses `candidate_cfg` as the final config.
- **Editable commands (`EditorOutcome::Abort`)** call `build` once for
  `pre_editor_cfg`, which `Ctx::new` reuses as the final config.
  (`Query` does not currently emit pre-open aborts.)

The editor flow consumes `PartialAppConfig` directly:

- `editor::edit_query` and `Query::build_conversation` take `&PartialAppConfig`
  instead of `&AppConfig`.
  `build_config_text` already converts its input via `to_partial()` to render
  the `[config]` preamble; with a partial input, it reads the fields directly.
- A new `PartialEditorConfig::command()` method mirrors
  `EditorConfig::command()`, reading `self.cmd: Option<String>` and `self.envs:
  Option<Vec<String>>` directly.
  The dispatch site passes `pre_editor_cfg.to_partial()` (already
  defaults-filled by `build`) so `PartialEditorConfig::command()` sees the
  resolved `envs`.

### Computing the editor delta

The editor preamble is pre-filled with values from the resolved partial, called
the *config seed* (`seed_partial`).
An unchanged preamble reproduces those values, so recording the parsed preamble
directly would persist seeded fields as if the user had edited them (for
example, a `--model` CLI flag would be recorded again as an editor-authored
`config_delta` event).

Instead, `Query::parse_editor_output` diffs the parsed preamble against the
config seed using `PartialConfigDelta::delta`:

```txt
editor_delta = seed_partial.delta(parsed_partial)
```

Only `editor_delta` is merged into the partial and persisted.
`editor::edit_query` currently skips this diff and records phantom deltas;
extracting `editor_delta` fixes that bug as part of moving the editor before
command dispatch.

**Kept drafts.** `QUERY_MESSAGE.md` is preserved across runs after an empty
query, an interrupt, or a failed turn.
When reopened, its preamble still contains the earlier invocation's config seed,
which may differ from the current invocation's seed (for example, when `--cfg`
selects a different model on the new run).
Diffing that stale preamble against the new `seed_partial` would treat every
unchanged pre-filled value from the earlier run as a user edit and record it.

To distinguish user edits from stale pre-filled values, the draft stores its
config seed in a hidden HTML comment below the preamble (`<!-- CONFIG_SEED: ...
-->`).
Before the editor opens, pre-filled fields in the preamble are reconciled
against the stored seed:

```txt
for field in pre-filled fields:
    if preamble[field] == stored_seed[field]:   // left alone by the user
        preamble[field] = seed_partial[field]
stored_seed := seed_partial                      // both written to the file
```

Reconciliation updates untouched pre-filled fields to the current invocation's
`seed_partial` while preserving any pre-filled fields the user modified.
The user sees the current invocation's config with their earlier edits layered
on top.

Fields outside the pre-filled set were added by the user and are kept verbatim,
including their merge strategy.
Running `seed_partial.delta(parsed_partial)` on those fields would diff them
against an empty seed and strip their merge strategy: an
`[assistant.instructions]` override with `strategy = "replace"` would be dropped
when empty, or converted to an append when non-empty.
Delta extraction therefore uses the same split as reconciliation: it computes
`seed_partial.delta(parsed_partial)` only over the pre-filled fields, and copies
every other field from `parsed_partial` into `editor_delta` as written.

Two edge cases apply when opening a kept draft:

- **Missing stored seed**: If a draft has no `<!-- CONFIG_SEED: ... -->`
  comment, pre-filled fields cannot be distinguished from user edits, so every
  pre-filled field is overwritten with `seed_partial` while user-added fields
  are kept.
- **Malformed TOML**: If the preamble is not valid TOML, it is left untouched
  along with its stored config seed, and the parse error reopens the editor via
  `ParseOutcome::Retry`.
  Once the user repairs the TOML, its pre-filled fields are reconciled against
  that stored seed so untouched values from the earlier run are not recorded.

**Limitations.** `seed.delta(parsed)` is additions-only:

- Deleting a seeded scalar field from the preamble produces no delta (per
  `delta_opt` semantics) rather than an unset directive.
- List-like fields (such as `conversation.attachments` and `conversation.tools`)
  are also additions-only: reordering or removing seeded list items does not
  produce a delta.
  The editor preamble cannot remove or reorder existing collection items.

Supporting removals and reorderings requires RFD 070's unset semantics.

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

Only `Query` implements `EditableCommand` today, with `EditorOutput =
PreparedQuery` (which wraps `ChatRequest` and editor-flow state; see "Trimmed
`Query::run`" below).
The trait decouples the editor protocol from command-specific types rather than
sharing implementation code across commands, since `editor_input` and
`parse_editor_output` are specific to each command.
`run_editor_protocol` is generic over `EditableCommand`, and the trait
signatures reference only `PartialAppConfig` and `Self::EditorOutput`.
`run_inner` still matches on `Commands::Query(q)` for enum dispatch, while query
domain types (`ChatRequest` and the `[config]/[history]` document format) remain
encapsulated in the `query` module.

`run_editor_protocol` drives the editor loop:

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

The retry loop preserves existing behavior: invalid TOML in the `[config]` block
re-renders the preamble with an inline error message and reopens the editor with
the user's edits intact.

`run_inner` dispatches editable commands before constructing `Ctx`:

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

For `Query`, `command.run` always invokes `Query::run` without special-casing
empty or aborted queries at dispatch.
`Query::run` handles `prepared = None` internally (see "Trimmed `Query::run`").

`drive_editor` wraps the existing `editor::open` plumbing and takes a structured
`EditorInput`:

```rust
struct EditorInput {
    path: Utf8PathBuf,           // QUERY_MESSAGE.md location
    seed: QueryDocument,         // initial preamble + body rendered to the file
    parse_error: Option<String>, // inline annotation rendered on retry
}
```

`RevertFileGuard` semantics remain unchanged: `drive_editor` writes the seed
content to `path` (creating `QUERY_MESSAGE.md` or reusing an existing draft
after reconciling its preamble as described in [Computing the editor
delta](#computing-the-editor-delta)), the guard restores the original file
content unless disarmed, and successful parsing disarms the guard before
returning.
On `ParseOutcome::Retry`, the protocol must preserve the user's edited file
contents for the next editor invocation, either by disarming the guard before
re-running the editor or by carrying the edited raw content into the next
`EditorInput`.
Reusing `path` alone is not enough if the guard restores the file between
attempts.

The existing `editor::edit_query` implementation splits across the two trait
methods:

- `Query::editor_input` builds `QueryDocument`, prepares the seed content, and
  reconciles kept drafts.
  The early-return paths in `Query::edit_message` (`--no-edit` with `--replay`,
  query passed via argv, or missing editor) either return `EditorRequest::Skip`
  with a `ChatRequest` built from argv, stdin, or replay, or return an error
  (such as `MissingEditor` when no editor is configured and no `ChatRequest` is
  available).
  `EditorRequest::Abort` is reserved for future commands and is not produced by
  `Query`.
- `Query::parse_editor_output` reparses the file via
  `QueryDocument::try_from(content)`, extracts `editor_delta` against the seed
  (see "Computing the editor delta" above), and returns `ParseOutcome::Retry`
  when the TOML preamble fails to parse.

Because command dispatch matches on the concrete `Commands` enum variant,
`EditableCommand` uses an associated `EditorOutput` type and static dispatch
rather than `Box<dyn EditableCommand>`.

### Pre-dispatch lock acquisition

The editor needs a conversation lock before opening: the lock provides both the
conversation directory for `QUERY_MESSAGE.md` and the event stream for rendering
the `[history]` preamble.
Acquiring the lock before constructing `Ctx` requires four adjustments:

- **`SignalPair` initialization**: `LockRequest` already accepts individual
  borrows (`workspace`, `handle`, `is_tty`, `session`, `printer`, `signals`, and
  `lock_wait`).
  Every dependency except `signals` already exists before `Ctx::new`.
  Moving `SignalPair::new(&runtime)` into `run_inner` (between `build_runtime`
  and lock acquisition) makes `signals` available to both `LockRequest` and
  `Ctx::new`.
- **Async execution**: `acquire_lock` is async and runs via `rt.block_on(...)`
  using the runtime created by `build_runtime`.
- **Decoupling helper functions from `Ctx`**: `Query::acquire_lock` calls
  `create_new_conversation(ctx)` and `fork_conversation(ctx, ...)` (the shared
  helper in `conversation/fork`), which both take `&mut Ctx`.
  Refactoring them to take their dependencies directly requires passing
  `(ctx.config(), ctx.workspace, ctx.session)` to `create_new_conversation` and
  `(ctx.now(), ctx.workspace, ctx.session)` to `fork_conversation`.
- **Seeding `--new` conversations**: `Workspace::create_and_lock_conversation`
  needs a resolved `AppConfig` to seed the new conversation's base config.
  `run_inner` passes `pre_editor_cfg` (from `build(partial.clone())`), avoiding
  any extra source loading.

### Trimmed `Query::run`

Editor invocation, `acquire_lock`, alias resolution, editor-delta recording, and
invocation-delta computation (`get_config_delta_from_cli`) all move from
`Query::run` into `run_inner`.
`Query::run` receives the acquired lock and `Option<PreparedQuery>`, and reads
the post-editor config from `ctx.config()`:

```rust
struct PreparedQuery {
    request: Option<ChatRequest>,
    query_file: Option<Utf8PathBuf>,
    opened_editor: bool,
}
```

- `prepared = None` represents a pre-open `EditorRequest::Abort`.
  `Query::editor_input` does not emit `Abort` today, but if reached,
  `Query::run` returns `Ok(())` immediately without side effects.
- `prepared = Some(pq)` is the normal `Query` path.
  `pq.request` holds `Some(ChatRequest)` when a query is ready to send, or
  `None` when the user left the editor body blank (possibly after editing the
  `[config]` preamble).
  `query_file` holds the `QUERY_MESSAGE.md` path for echoing and cleanup, and
  `opened_editor` distinguishes editor-produced requests from CLI or stdin
  queries.

When `prepared` is `Some(pq)`, `Query::run` always performs:

- `--title` metadata update
- session activation
- pre-query compaction (when `--compact` is set)

If `pq.request` is `None`, `Query::run` prints "Query is empty, ignoring.",
removes `pq.query_file`, and returns without starting MCP servers (see Risks).

If `pq.request` is `Some(_)`, `Query::run` proceeds with the turn:

- `configure_active_mcp_servers`
- title task spawn
- tool definition and attachment loading
- thread construction and `handle_turn`

`query_file` cleanup matches existing behavior: the file is removed after a
successful turn or an empty edited query, and **preserved on turn error** so the
user can recover their draft from `QUERY_MESSAGE.md`.

### Finalization for pre-`Ctx` errors

`run_inner` currently finalizes execution after `command.run` using `ctx`: it
flushes the printer, syncs `task_handler`, calls
`remove_ephemeral_conversations`, and runs `cleanup_stale_files`.
With editor invocation moved before `Ctx::new`, errors can occur before `Ctx`
exists (during `build(partial)`, lock acquisition, or editor execution).

To handle both stages, `run_inner` splits finalization into two helpers:

- `post_ctx_finalize(&mut ctx)` runs the full finalization sequence after
  `Ctx::new`.
- `pre_ctx_finalize(&mut workspace, &printer, fs_backend)` flushes the printer
  and runs workspace cleanup (`remove_ephemeral_conversations` and
  `cleanup_stale_files`) using locals already in scope, skipping
  `task_handler.sync(...)` because `Ctx` (which owns `task_handler`) has not
  been created and no background tasks have been spawned.

Pre-`Ctx` error paths in `run_inner` route through `pre_ctx_finalize` (via an
RAII guard or `map_err`), while post-`Ctx` paths call `post_ctx_finalize`.

### MCP server activation timing

`configure_active_mcp_servers` currently runs before the editor opens, booting
MCP servers while the user types.
Moving the editor before `Ctx::new` delays MCP activation until after the editor
closes, which adds 1 to 2 seconds of startup latency after long edit sessions.

Running MCP activation after the editor also fixes a bug: because `mcp_client`
and the active tool list are now built from the post-editor `AppConfig`, editor
changes to `[providers.mcp.*]` and `[conversation.tools.*]` take effect on the
current turn instead of being ignored until the next invocation.

## Non-Goals

- Relaxing `Ctx::config` immutability.
- Changing the `config_delta` event schema or how deltas are stored.
- Generalizing mid-run configuration sources beyond the editor.
- Booting MCP servers in parallel with the editor.
  If startup latency becomes noticeable, a follow-up can construct `mcp_client`
  before `Ctx::new` from the pre-editor partial and pass it into `Ctx::new`.

## Alternatives

We considered and rejected two smaller changes that avoid restructuring the
configuration pipeline:

- **Rebinding `cfg` locally in `Query::run` after the editor**: Fixes the
  immediate bug for local reads in `Query::run`, but leaves `ctx.config()`
  returning the stale pre-editor snapshot.
  Any post-editor code that reads `ctx.config()` would silently use outdated
  values.
- **Adding a `Ctx::refine_config` setter**: Breaks the invariant that `Ctx`
  configuration is immutable after initialization, and leaves `mcp_client`
  (constructed once in `Ctx::new` from `config.providers.mcp`) bound to the
  pre-editor config.

Both approaches treat the editor as a special case rather than a configuration
source.
Running the editor before `Ctx::new` folds it into the standard resolution
pipeline, at the cost of restructuring `run_inner` and lock acquisition.

## Risks

- **`fork_conversation` and `create_new_conversation` signature changes**: Both
  functions currently take `&mut Ctx`.
  Decoupling them from `Ctx` updates their call sites in `query` and the
  conversation subcommands (`archive`, `compact`, `edit`, and `rm`).
  Subcommands that neither fork nor create conversations continue to use
  `LockRequest::from_ctx` unchanged.
- **`EditableCommand` trait stability**: `EditableCommand` has a single
  implementor (`Query`), so its shape may change when a second command adopts
  it.
  `EditorRequest` keeps the dispatch contract narrow.
- **`Commands::run` signature for `Query`**: `Query::run` takes `(lock,
  Option<PreparedQuery>)` from the dispatch layer while other command variants
  do not.
  Because `Commands::run` dispatches via a `match` on each variant, this change
  is confined to the `Commands::Query` arm.
- **MCP startup timing**: Running MCP activation after the editor closes removes
  the 1 to 2 second overlap with user typing on long edit sessions, and skips
  MCP startup altogether when an edited query is empty (whereas today's pipeline
  boots MCP servers even when no query is sent).
  Both trade-offs are acceptable initially and can be revisited if startup
  latency becomes an issue.
- **Pre-`Ctx` error reporting**: Editor errors now return from `run_inner`
  instead of flowing through `cmd::Error` from `Query::run`.
  Because the printer and error formatting machinery are initialized before
  `Ctx::new`, user-visible error output remains the same; the test suite will
  verify this with `MissingEditor`.
- **Template rendering on both editor paths**: Template rendering continues to
  use `template.values` from the pre-editor partial, so template values added in
  the editor preamble do not apply to the current turn (matching existing
  behavior).
  Because `build_conversation` currently runs `if self.template` unconditionally
  after `edit_message`, both `EditorRequest::Skip` and `ParseOutcome::Run` must
  render templates before returning (see Implementation Plan step 6).
- **Pre-editor failure side effects**: Failures during `edit_message` (such as
  `MissingEditor`) currently occur inside `Query::run` after session activation,
  `--title` updates, `--compact` compaction, and `configure_active_mcp_servers`.
  Moving the editor before `Query::run` means those side effects no longer run
  when the editor fails to open.
  Skipping session, title, and MCP setup when no turn can proceed is desirable,
  but it is a behavioral change.

## Implementation Plan

1. Fix `jp_config::delta::delta_opt_vec` for `Vec` fields that rely on it.
   It currently returns `None` whenever every element of `prev` is contained in
   `next`, even if `next` has additional elements (missing a `prev.len() ==
   next.len()` check), which silently drops additions.
   `conversation.attachments` uses separate additions-only logic in
   `PartialConversationConfig::delta` and is unaffected by `delta_opt_vec`, but
   should be covered by the same seed-vs-parsed tests because editor-added
   attachments are the motivating case for this RFD.
2. Split `resolve_config` into `resolve_partial` (returning `(PartialAppConfig,
   Vec<ConversationHandle>)`) and a `build(partial)` call in `run_inner`.
3. Add `PartialEditorConfig::command()` mirroring `EditorConfig::command()`,
   with unit tests verifying parity.
4. Move `SignalPair::new(&runtime)` from `Ctx::new` into `run_inner` and pass it
   to both the pre-dispatch helpers and `Ctx::new`.
5. Refactor `fork_conversation` and `create_new_conversation` to take individual
   dependencies instead of `&mut Ctx`, keeping existing fork tests passing.
6. Define `EditableCommand`, `EditorRequest`, `ParseOutcome`, and
   `EditorOutcome`, and implement `EditableCommand` for `Query`:
   - `editor_input` relocates `build_conversation`'s stdin, argv, replay, and
     seed-building logic, reconciles any kept draft's preamble against its
     stored config seed, and returns `EditorRequest<PreparedQuery>`.
     The query-as-argv and `--no-edit` paths return `Skip` after rendering
     templates on the output; `Abort` is unused by `Query`.
   - `parse_editor_output` parses the saved preamble, computes `editor_delta =
     seed.delta(parsed)`, renders templates on the edited content, and returns
     `ParseOutcome::Run { delta, output }` (or
     `ParseOutcome::Retry(EditorInput)` with an inline error annotation when
     TOML parsing fails).
     Empty edited queries return `Run` with `output.request: None` so
     `query_file` is preserved for cleanup.
     Rendering templates in both `editor_input` (`Skip`) and
     `parse_editor_output` (`Run`) preserves the unconditional `if self.template
     { ... }` pass in `build_conversation`.
7. Add `run_editor_protocol` with an internal retry loop on
   `ParseOutcome::Retry`, and wire editable commands into `run_inner` after
   `resolve_partial` and signal initialization:
   - Extract the query's move-only `ConversationHandle` from `handles`.
   - Build `pre_editor_cfg`, acquire the lock (using `pre_editor_cfg` to seed
     `--new` conversations), compute `invocation_delta` from
     `pre_editor_cfg.to_partial()`, and **record `invocation_delta`
     immediately** so a subsequent editor failure still persists CLI intent.
   - Run the editor protocol.
     On `EditorOutcome::Run`, build `candidate_cfg`, resolve `editor_delta`
     aliases against `candidate_cfg`'s alias map, record `editor_delta` if
     non-empty, and use `candidate_cfg` as the final config.
     On `EditorOutcome::Abort`, reuse `pre_editor_cfg` as the final config.
   - Construct `Ctx::new` and invoke `command.run` unconditionally (`Query::run`
     handles empty queries internally).
   - Route pre-`Ctx` errors through `pre_ctx_finalize`.
8. Update `Query::run` to accept `(lock, Option<PreparedQuery>)` and remove
   editor invocation, `acquire_lock`, alias resolution, editor-delta recording,
   and `get_config_delta_from_cli` (all handled in `run_inner`):
   - On `prepared = None` (pre-open abort, unused by `Query`), return `Ok(())`
     immediately.
   - On `Some(pq)` with `pq.request.is_none()` (post-edit empty query), run
     `--title`, session activation, and `--compact`, print "Query is empty,
     ignoring.", remove `pq.query_file`, and return.
   - On `Some(pq)` with `pq.request.is_some()`, run the full turn, removing
     `query_file` on success and preserving it on error.
9. Verify end-to-end:
   - All existing query paths (`jp q`, `jp q "msg"`, `jp q --new`, `jp q
     --fork`, `jp q --replay`, and `jp q --no-edit`) preserve current behavior,
     except that empty edited queries skip MCP startup.
   - Editor-added `conversation.attachments` take effect on the same query turn
     (the regression motivating this RFD).
   - Combining CLI `--attachment` with an editor-added attachment preserves the
     expected order on the next invocation (verifying that `invocation_delta` is
     recorded before `editor_delta`).
   - `jp q --new` with an editor-provided model uses that model on the same turn
     (covering the `--new` two-build path).
   - Leaving the editor preamble unchanged after `--model X` produces no phantom
     `config_delta` event (covering seed-vs-parsed delta extraction).
   - Reopening a draft kept after a failed turn under a different config shows
     the current invocation's config with only the user's earlier edits on top,
     and records only those edits (covering preamble reconciliation).
   - `jp q --model alias` persists the resolved model ID in the invocation delta
     event rather than the alias name (covering invocation-delta alias
     resolution).
   - Semantically invalid editor config (such as an unknown provider or
     unresolved alias) leaves the editor delta unrecorded while keeping any
     non-empty `invocation_delta` that was already recorded before the editor
     opened.
   - Defining and using a new model alias in the same editor preamble resolves
     the alias for the current turn and persists the resolved model ID.
   - Deleting a seeded field from the editor preamble is a no-op.
   - `jp q --template "..." "message"` (the `Skip` path, where no editor opens)
     still renders templates against `template.values` and rejects undefined
     variables, matching `build_conversation`.
   - A post-edit empty query (`PreparedQuery` with `request: None`) removes
     `QUERY_MESSAGE.md`, releases the lock, and runs standard `run_inner`
     finalization (printer flush, task sync, and ephemeral cleanup).
   - A turn error after a non-empty editor query preserves `QUERY_MESSAGE.md` so
     the user can recover their composed text.
   - Invalid TOML in the editor's `[config]` block reopens the editor with the
     user's edits preserved and the parse error annotated inline (covering
     `ParseOutcome::Retry` and the `RevertFileGuard` lifecycle on retry).
   - A pre-`Ctx` error (such as `MissingEditor`) runs `pre_ctx_finalize` to
     flush the printer and clean up stale workspace files, and renders the same
     user-facing error message as before.

Steps 1 through 6 are independently mergeable refactors.
Steps 7 and 8 land the behavior change, and step 9 verifies it end-to-end.

## References

- RFD 054: `config_delta` event semantics reused here.
- RFD 070: Source-tagged claims.
  If RFD 070 lands first, the editor delta is emitted as `ApplyDelta` with
  source `editor:query`, the invocation delta uses `cli:flag` or `cli:cfg` per
  directive, and for `--new` the tentative `AppConfig` build is validation-only
  while conversation creation uses RFD 070's `base + init` partition with these
  deltas as init entries.
  If this RFD lands first, events carry no claims and RFD 070 treats them as
  legacy events.
- RFD 079: Configuration source and precedence model that this RFD extends.
- Issue #217: Original `QUERY_MESSAGE.md` configuration surface proposal.
- Issue #91: Broader query-editor protocol discussion.

[RFD 079]: 079-config-sources-and-load-order.md
