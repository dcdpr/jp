# Conversation config is resolved before the lock and never re-read after a wait

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-31

## What happens

The per-conversation config layer is resolved during bootstrap, before any
command acquires a conversation lock.
A command that then waits for the flock runs with config that predates whatever
the lock holder recorded while it waited.

`jp_cli/src/lib.rs:948-967` is phase 2 of the config pipeline: it eager-loads
the conversation (`eager_load_conversation`, line 951), folds the stream's
config deltas into the partial, and hands the result to
`ConfigPipeline::partial_with_conversation`.
Every command that resolves conversation config goes through this, and it
happens well before `acquire_lock`.

So if the lock holder appends a config delta during its turn — a model change,
a tool toggle, a compaction rule — the waiting command's `ctx.config()` does
not see it.

## Severity

This is wrong-config, not data loss.
`Workspace::lock_conversation` re-reads the event stream and the write
projection at acquisition, so the *stream* the command mutates is current; only
the resolved `AppConfig` derived from it is stale.

Visible effects depend on the command: `jp c compact` could summarize with the
model the conversation had before the other process switched it, or apply
compaction rules the conversation no longer configures.
The stale config is not persisted as a delta, so the damage is confined to the
waiting command's own run.

## Why it is not a small fix

The config pipeline runs once, before the command's `run` is called, and its
output is threaded through `Ctx`.
Re-resolving after lock acquisition means either:

- re-running phase 2 and phase 3 from inside the command, once the lock is held
  (each command would need to opt in, and `Ctx::config` would need to become
  replaceable), or
- moving lock acquisition ahead of config resolution for commands that write,
  which reorders bootstrap and changes when contention prompts appear relative
  to config errors.

Both are structural.
Worth checking against RFD 074 (Eager Loading with Command-Declared Data
Requirements), which already proposes declaring per-command data needs up front
and would give the ordering a natural home.

## Origin

Noticed while fixing the stale-event-stream data loss in \#1045
(`Workspace::lock_conversation` now re-reads metadata, events, and the write
projection once the flock is held).
The config layer is the remaining thing read before the lock and not refreshed
after it.
