# RFD D40: Background Task Infrastructure

- **Status**: Draft
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22

## Summary

The `jp_task` crate provides JP's bounded background task primitive: async work
that runs alongside a command and gets one best-effort opportunity to commit
results to the workspace before JP exits.
Use it for work that should not block the user's query and whose result is
droppable if the task cannot finish within the shutdown budget.
Task errors are logged and dropped; they never fail the parent command.

## When to use a background task

Spawn a background task when:

- The work is genuinely independent of the user's immediate response (title
  generation, garbage collection, refresh sweeps).
- The work is cancellable and its result is droppable — losing the result is
  acceptable if the task cannot finish within the shutdown budget.
- Its result, when it does complete, should be written to the workspace before
  JP exits and need not be visible to the user mid-query.

Do not use it for:

- Work the user is waiting on directly.
  Stay on the main async path.
- Work that must survive across JP invocations, process kill, timeout
  cancellation, or task failure.
  Background task results are best-effort — use a durable mechanism that
  survives process exit instead.
- Work whose lifetime is already awaited by the parent command and whose result
  is safe to drop at runtime shutdown.
  A plain `tokio::spawn` is fine when you don't need `TaskHandler`'s drain and
  cancellation semantics.

## The `Task` trait

Implement `jp_task::Task` for every background task:

```rust
#[async_trait]
pub trait Task: Send + 'static {
    fn name(&self) -> &'static str;

    async fn run(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>;

    async fn sync(
        self: Box<Self>,
        ctx: &mut Workspace,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}
```

The contract is two-phase:

- **`run` (background phase)** receives a `CancellationToken` and runs
  concurrently with the rest of the command.
  It must observe the token and return promptly when cancelled.
  On success it returns `self` — the same task, carrying whatever accumulated
  results — so the sync phase can commit them.
  The token is a child of the `TaskHandler`'s root token, so a single shutdown
  cancels every live task.

- **`sync` (workspace phase)** runs after `run` returns, with exclusive `&mut
  Workspace`.
  This is where the task commits its results — writes a title, deletes a stale
  file, updates metadata.
  If you have no result to commit, leave `sync` as the default no-op.

Returning `Self` from `run` is intentional: it lets the run phase accumulate
state in struct fields and hand it off to `sync` typed, without serializing
through channels.

## The `TaskHandler`

`TaskHandler` (in `jp_task::handler`) owns a `tokio::task::JoinSet` and a root
`CancellationToken`.
It exposes two methods:

- **`spawn(task)`** — enqueues the task on the `JoinSet` and starts it.
  Returns immediately.
- **`sync(workspace, timeout)`** — drains the `JoinSet`.
  Within `timeout`, waits for each task to finish its `run` phase; on success it
  calls `task.sync(workspace)`.
  If the timeout elapses, it cancels the root token (signalling all live `run`s
  to stop), then gives them a fixed 2-second grace window before
  force-shutting-down the `JoinSet`.
  Tasks that do not respond to cancellation lose their accumulated results.
  The `timeout` applies to the `run` phase; `task.sync` runs afterwards with no
  handler-level deadline (see Boundaries).

The two-stage drain (timeout → cancel → grace → force-shutdown) means tasks
should treat `CancellationToken` as a hard deadline, not a hint.
A `run` that polls the token only occasionally will be force-killed without
`sync` running.

`TaskHandler` is `Default` and lives on `Ctx::task_handler` in `jp_cli::ctx`.
Every command has access to it via the shared context.

## Integration point

The normal command pipeline drains tasks at exactly one place: the end of
`jp_cli::lib::run`, after `cli.command.run()` returns and before ephemeral
conversation cleanup.
Commands that bypass `Ctx` construction (e.g. `jp init`) do not run a drain —
they cannot spawn tasks either.

```rust
// Wait for background tasks to complete and sync their results to the workspace.
rt.block_on(
    ctx.task_handler.sync(&mut ctx.workspace, Duration::from_secs(10)),
).map_err(Error::Task)?;
```

The 10-second budget is the soft deadline for `run` before cancellation; the
hard deadline is +2 seconds after that.
The `sync` phase that follows has no handler-level timeout — `sync` bodies are
expected to be small (see Boundaries).
A command can spawn tasks from anywhere it has `&mut Ctx`:

```rust
ctx.task_handler.spawn(TitleGeneratorTask::new(cid, stream, &cfg)?);
```

There is no per-command sync.
Tasks live for the duration of the `jp` process.
A query that spawns a task and then errors out still gets the task drained on
exit.

## Concrete tasks

Two implementations ship today, both under `jp_task::task`:

- **`StatelessTask`** wraps a `Future<Output = Result<(), Error>>` and runs it
  under the cancellation token.
  The `sync` is a no-op.
  Use it as an escape hatch for work that needs `TaskHandler`'s drain and
  cancellation semantics but has no workspace mutation to commit.
  It has no current callers in the tree — `tokio::spawn` is the default when
  you don't need those semantics.

- **`TitleGeneratorTask`** generates a conversation title via the LLM during
  `run` and writes it to conversation metadata during `sync`.
  The `sync` reacquires a lock on the conversation
  (`Workspace::lock_conversation`) before writing, since the main query's lock
  has been released by the time `sync` runs.

`TitleGeneratorTask` is the canonical example of the trait's two-phase shape:
the heavy work (LLM call) happens in `run` while the user is doing something
else, and the workspace mutation happens in `sync` under a fresh lock.

## Boundaries

- **Locks.** The main command holds workspace locks during `run`.
  By the time `sync` executes, those locks have been released.
  If your task mutates a conversation, `sync` must reacquire its own lock — and
  handle the case where another session is now holding it (skip and log; do not
  block).
- **`sync` is unbounded — keep it small.** The shutdown budget applies only to
  the `run` phase.
  Once `run` returns, `task.sync` is awaited sequentially with no handler-level
  deadline.
  Heavy work — scans, LLM calls, network I/O, retry loops — belongs in `run`,
  not `sync`.
  A `sync` body must be small, local, non-networked, and non-blocking; lock
  acquisition must be non-blocking or internally bounded (see the Locks bullet).
  Anything else can stall JP at process exit.
- **Errors in tasks** are logged via `tracing` and dropped — they do not
  propagate out of `TaskHandler::sync`.
  A failing background task never fails the parent command.
- **Tasks cannot spawn tasks.** A task has no handle to `TaskHandler`; fan-out
  happens inside a single `Task`, not across multiple.
  Internal fan-out via `tokio::spawn` is allowed, but the parent task must own
  any child whose result feeds back into `sync`: pass cancellation tokens to
  children and await or abort them before returning `Ok(self)` from `run`.
  Detached spawns are only legitimate when the child's result is intentionally
  disposable (e.g. fire-and-forget signalling forwarders).
- **`--no-persist`.** When `ctx.term.args.persist` is `false`, writes are no-ops
  via the null backends.
  Tasks whose only purpose is mutating the workspace (e.g. title generation)
  should be suppressed at spawn time by the caller; `TaskHandler` itself does
  not gate on `persist`.
- **Command errors and persistence.** Task `sync` runs after command error
  handling.
  Most command errors default to disabling persistence (`jp query` opts in for
  turn errors via `.with_persistence(true)`).
  When persistence is disabled on the error path, `ctx.workspace` is swapped to
  `NullPersistBackend` *before* `task_handler.sync` runs.
  Any `ConversationMut` a task acquires inside `sync` then writes through the
  null backend and silently no-ops.
  Task authors who need commit-on-error must rely on the command opting in to
  persistence on its error path, or commit during `run` rather than `sync`.

## Adding a new task

1. Define a struct carrying the inputs the task needs (IDs, configs, any
   `Arc<dyn LoadBackend>` clones for read-only access) and the outputs you will
   commit in `sync`.
2. Implement `Task` for it.
   The `run` body must observe the cancellation token — use `jp_macro::select!`
   against `token.cancelled()`.
3. Place the task with the domain it mutates or next to its single caller.
   The `Task` trait and `TaskHandler` live in `jp_task`; concrete tasks should
   not — `jp_task` should not grow a new domain dependency for every new task.
   Promoting a task into `jp_task::task` is reserved for tasks that are stable,
   reused across multiple callers, and do not pull a new crate dependency into
   `jp_task`.
4. Spawn it via `ctx.task_handler.spawn(MyTask { ... })` from the command that
   wants the work done.

## Related RFDs

In this guide, "background task" means a `jp_task::Task` managed by
`TaskHandler`.
"Async task" or plain `tokio::spawn` refers to turn-local concurrent work that
is not drained by `TaskHandler` — the structured inquiry tasks in [RFD 028] are
of this second kind, not `jp_task` users.

Current `jp_task::Task` users:

- `TitleGeneratorTask` — generates a conversation title on the first turn
  during `run` and writes it during `sync`.

Designs that intend to use this primitive (Discussion status; subject to
change):

- [RFD 053] — `TitleRefreshTask` extends the title-generation pattern with
  periodic re-evaluation.
- [RFD 066] — the blob-store garbage collection sweep.

New RFDs that depend on this primitive should declare `Requires: RFD NNN` on
this RFD once it is published.
That places it in the dependency graph and surfaces a back-link here.

[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 053]: ../053-auto-refresh-conversation-titles.md
[RFD 066]: ../066-content-addressable-blob-store.md
