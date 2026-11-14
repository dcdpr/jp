# RFD 107: Background Task Infrastructure

- **Status**: Implemented
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22
- **Summary**: `jp_task` provides bounded background task primitives for
  concurrent work that commits to the workspace before exit.

## Summary

The `jp_task` crate provides JP's bounded background task primitive: async work
that runs alongside a command and gets one best-effort opportunity to commit
results to the workspace before JP exits.
Use it for work that should not block the user's query and whose result can be
dropped if the task does not finish within the drain timeout (the time
background tasks are given to finish after the command returns).
Task errors are logged and dropped; they never fail the parent command.

## When to use a background task

Spawn a background task when:

- The work is independent of the user's immediate response (title generation,
  garbage collection, refresh sweeps).
- The work is cancellable, and losing its result is acceptable if the task does
  not finish within the drain timeout.
- Completed results must be written to the workspace before JP exits, but do not
  need to be visible to the user mid-query.

Do not use it for:

- Work the user is waiting on directly.
  Stay on the main async path.
- Work that must survive across JP invocations, process termination, timeout
  cancellation, or task failure.
  Background task results are best-effort; use durable storage when work must
  survive process exit.
- Work whose lifetime is already awaited by the parent command and whose result
  is safe to drop at runtime shutdown.
  Use a plain `tokio::spawn` when you do not need `TaskHandler`'s drain and
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

The contract has two phases:

- **`run` (background phase)** receives a `CancellationToken` and runs
  concurrently with the command.
  The token is a child of `TaskHandler`'s root token, so a single shutdown
  cancels every live task.
  `run` must observe the token and return promptly when cancelled.
  On success, it returns `self` carrying any accumulated results for the sync
  phase to commit.

- **`sync` (workspace phase)** runs after `run` completes, with exclusive `&mut
  Workspace` access.
  It commits the task's results to the workspace, such as writing a conversation
  title, deleting a stale file, or updating metadata.
  Tasks with nothing to commit can use the default no-op implementation.

Returning `Self` from `run` lets the background phase accumulate state in struct
fields and pass it to `sync` with its concrete type intact, without serializing
through channels.

## The `TaskHandler`

`TaskHandler` (in `jp_task::handler`) owns a `tokio::task::JoinSet` and a root
`CancellationToken`.
It exposes two methods:

- **`spawn(task)`**: Enqueues the task on the `JoinSet`, starts it, and returns
  immediately.
- **`sync(workspace, timeout)`**: Drains the `JoinSet`.
  It waits up to `timeout` for each task to finish `run`, calling
  `task.sync(workspace)` as each succeeds.
  If `timeout` elapses, it cancels the root token to stop all running tasks,
  waits a fixed 2-second grace period, and then shuts down the `JoinSet`.
  The `timeout` bounds only the `run` phase; `task.sync` runs afterward with no
  handler-level deadline (see Boundaries).

Because the two-stage drain (timeout → cancel → grace → force-shutdown)
aborts any task still running after the grace period, tasks must treat
`CancellationToken` as a hard deadline rather than a hint.
A `run` implementation that polls the token infrequently risks being killed
before `sync` runs and losing its accumulated results.

`TaskHandler` implements `Default` and lives on `Ctx::task_handler` in
`jp_cli::ctx`, so every command can access it via the shared context.

## Integration point

Tasks live for the duration of the `jp` process; there is no per-command sync.
The command pipeline drains tasks once, at the end of `jp_cli::lib::run`, after
`cli.command.run()` returns and before ephemeral conversation cleanup.
Even if a query spawns a task and then returns an error, the handler still
drains the task on exit.
Commands that bypass `Ctx` construction (such as `jp init`) neither spawn nor
drain tasks.

```rust
// Wait for background tasks to complete and sync their results to the workspace.
rt.block_on(
    ctx.task_handler.sync(&mut ctx.workspace, Duration::from_secs(10)),
).map_err(Error::Task)?;
```

The 10-second drain timeout is the soft deadline for `run` before cancellation,
and the hard deadline is 2 seconds later.
The `sync` phase that follows has no handler-level timeout, so `sync` bodies
must stay small (see Boundaries).

A command can spawn tasks from anywhere it holds `&mut Ctx`:

```rust
ctx.task_handler.spawn(TitleGeneratorTask::new(cid, stream, &cfg)?);
```

## Concrete tasks

Two implementations live under `jp_task::task`:

- **`StatelessTask`** wraps a `Future<Output = Result<(), Error>>` and runs it
  under the cancellation token, with a no-op `sync`.
  Use it as an escape hatch for work that needs `TaskHandler`'s drain and
  cancellation semantics but has no workspace mutation to commit.
  It has no current callers in the tree; `tokio::spawn` is the default when
  those semantics are not needed.

- **`TitleGeneratorTask`** is the canonical example of the trait's two-phase
  shape.
  It performs the heavy work (generating a conversation title via the LLM)
  during `run` while the command continues, and writes the title to conversation
  metadata during `sync`.
  Because the main query releases its conversation lock before `sync` runs,
  `sync` reacquires a fresh lock (`Workspace::lock_conversation`) before
  writing.

## Boundaries

- **Locks.** The main command holds workspace locks during `run` and releases
  them before `sync` executes.
  If a task mutates a conversation, `sync` must reacquire its own lock and
  handle contention when another session holds it (skip and log; do not block).
- **Unbounded `sync` phase.** The drain timeout applies only to `run`.
  Once `run` returns, `task.sync` is awaited sequentially with no handler-level
  deadline.
  Heavy work (scans, LLM calls, network I/O, retry loops) belongs in `run`, not
  `sync`.
  A `sync` body must be small, local, non-networked, and non-blocking, and lock
  acquisition must be non-blocking or internally bounded (see the Locks bullet).
  Anything else can stall JP at process exit.
- **Task errors.** `TaskHandler::sync` logs task errors via `tracing` and drops
  them rather than propagating them.
  A failing background task never fails the parent command.
- **Tasks cannot spawn tasks.** A task has no handle to `TaskHandler`; fan-out
  happens inside a single `Task`, not across multiple tasks.
  Internal fan-out via `tokio::spawn` is allowed, but the parent task must own
  any child whose result feeds into `sync`: pass cancellation tokens to children
  and await or abort them before returning `Ok(self)` from `run`.
  Detached spawns are only valid when the child's result is disposable (such as
  fire-and-forget signalling forwarders).
- **`--no-persist`.** When `ctx.term.args.persist` is `false`, writes become
  no-ops via the null backends.
  `TaskHandler` does not gate on `persist`, so callers should skip spawning
  tasks whose only purpose is mutating the workspace (such as title generation).
- **Command errors and persistence.** Task `sync` runs after command error
  handling.
  Most command errors disable persistence by swapping `ctx.workspace` to
  `NullPersistBackend` before `task_handler.sync` runs (`jp query` opts in for
  turn errors via `.with_persistence(true)`).
  Any `ConversationMut` a task acquires inside `sync` then writes through the
  null backend and silently no-ops.
  Tasks that must commit on error either rely on the command opting in to
  persistence on its error path, or commit during `run` rather than `sync`.

## Adding a new task

1. Define a struct carrying the task's inputs (IDs, configs, and any `Arc<dyn
   LoadBackend>` clones for read-only access) and the outputs to commit in
   `sync`.
2. Implement `Task` for it.
   The `run` body must observe the cancellation token by polling
   `token.cancelled()` inside `jp_macro::select!`.
3. Place the task with the domain it mutates or next to its single caller.
   The `Task` trait and `TaskHandler` live in `jp_task`, while concrete tasks
   live outside it so `jp_task` does not gain a new domain dependency for every
   task.
   Reserve `jp_task::task` for tasks that are stable, reused across multiple
   callers, and do not pull new crate dependencies into `jp_task`.
4. Spawn the task from the command via `ctx.task_handler.spawn(MyTask { ... })`.

## Related RFDs

In this guide, "background task" means a `jp_task::Task` managed by
`TaskHandler`.
"Async task" or plain `tokio::spawn` refers to turn-local concurrent work that
`TaskHandler` does not drain, such as the structured inquiry tasks in [RFD 028].

Current `jp_task::Task` users:

- `TitleGeneratorTask`: Generates a conversation title on the first turn during
  `run` and writes it during `sync`.

Designs that intend to use this primitive (Discussion status; subject to
change):

- [RFD 053]: `TitleRefreshTask` extends the title-generation pattern with
  periodic re-evaluation.
- [RFD 066]: The blob-store garbage collection sweep.

New RFDs that depend on this primitive should declare `Requires: RFD NNN` on
this RFD once it is published.
This records the dependency and surfaces a back-link here.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 066]: 066-content-addressable-blob-store.md
