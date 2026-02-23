# RFD 005: Turn-Scoped Stream Mutations

- **Status**: Draft
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-18

## Summary

This RFD proposes restricting `ConversationStream` mutations to go through
turn-scoped accessors (`start_turn` and `current_turn_mut`), making it
impossible to construct a stream without proper turn boundaries through the
public API.

## Motivation

`ConversationStream` currently exposes raw `push`, `add_chat_request`,
`add_chat_response`, and similar methods that allow callers to add events in
any order. Nothing prevents creating a stream without a `TurnStart`, adding a
`ChatResponse` before a `ChatRequest`, or pushing events outside a turn
boundary.

This has caused real bugs:

- Fork with `--from`/`--until` can produce streams that start with assistant
  responses, causing provider API errors (Anthropic rejects streams where the
  first message isn't from the user).
- Streams without `TurnStart` markers break `--last` (which counts turn
  boundaries).
- `sanitize()` exists specifically to repair these invariant violations after
  the fact.

We added `sanitize()` as a caller-side guard, but it's a patch — callers must
remember to call it, and the raw methods remain available for anyone to misuse.

## Design

### Public API

Two new entry points on `ConversationStream` replace direct event pushing:

```rust
// Start a new turn. Atomically adds TurnStart + ChatRequest.
stream.start_turn(chat_request);

// Get a mutable handle to the current (last) turn.
// If no turn exists, one is injected automatically.
let turn = stream.current_turn_mut();
turn.add_chat_response(response)
    .add_tool_call_request(req)
    .build()?;
```

`current_turn_mut()` is infallible — if the stream has no turns yet, it injects
an empty `TurnStart` and returns a handle to it. This avoids forcing every call
site to handle a `None`/`Err` for a case that shouldn't happen in practice.

### `TurnMut<'_>`

`TurnMut` wraps `&mut ConversationStream` and buffers events internally.
Events are validated, sanitized, and flushed to the stream when `build()` is
called. This keeps the stream in a consistent state at all times — partial or
invalid events never appear on the stream.

The turn loop works naturally with this model because `TurnMut` is short-lived:
grab a handle, add events, `build()`, release. Code that reads the stream
(e.g., `build_thread` cloning the stream for each LLM cycle) always runs
between `build()` calls, never while a `TurnMut` is held.

Two method styles for ergonomics:

- **`with_xxx(&mut self, x: X) -> &mut TurnMut`** — borrowed, for chaining
  on an existing binding.
- **`add_xxx(mut self, x: X) -> TurnMut`** — owned, for fluent builder chains.
- **`build(self) -> Result<()>`** — validates the buffered events, sanitizes
  them, flushes to the stream, and releases the borrow.

All methods are `#[must_use]` to prevent silently dropping a `TurnMut` without
calling `build()`. There is no custom `Drop` implementation.

Exposes:

- `add_chat_request` / `with_chat_request` — for interrupt replies within a turn
- `add_chat_response` / `with_chat_response`
- `add_tool_call_request` / `with_tool_call_request`
- `add_tool_call_response` / `with_tool_call_response`
- `add_inquiry_request` / `with_inquiry_request`
- `add_inquiry_response` / `with_inquiry_response`
- `build` — validate and commit

Does **not** expose:

- `add_turn_start` — only `start_turn()` can create turn boundaries
- `push` — raw event insertion is internal-only

### Visibility changes

| Method | Current | Proposed |
|--------|---------|----------|
| `start_turn` | N/A (new) | `pub` |
| `current_turn_mut` | N/A (new) | `pub` |
| `push` | `pub` | `pub(crate)` |
| `add_chat_request` | `pub` | `pub(crate)` |
| `add_chat_response` | `pub` | `pub(crate)` |
| `add_turn_start` | `pub` | `pub(crate)` |
| `add_tool_call_*` | `pub` | `pub(crate)` |
| `add_inquiry_*` | `pub` | `pub(crate)` |
| `sanitize` | `pub` | `pub` (unchanged) |
| `retain`, `iter`, etc. | `pub` | `pub` (unchanged) |

Internal code (`sanitize`, `trim_trailing_empty_turn`) continues to access
`self.events` directly. The builder guards the public API, not internal repair
logic.

### Caller migration

**`TurnCoordinator::start_turn`** — currently calls `add_turn_start` +
`add_chat_request`. Becomes `stream.start_turn(request)`.

**`TurnCoordinator` event handlers** — currently call `stream.add_chat_response`,
`stream.add_tool_call_response`, etc. Become
`stream.current_turn_mut().with_chat_response(...).build()?`. The `TurnMut` is
grabbed and consumed within each synchronous block, avoiding borrow conflicts
across async boundaries.

**`InterruptAction::Reply`** — adds a `ChatRequest` mid-turn via
`stream.add_chat_request(...)`. Becomes
`stream.current_turn_mut().with_chat_request(...).build()?`. This is valid — a
reply within a turn is a `ChatRequest` that doesn't start a new turn.

**Fork** — uses `extend` and `retain` on the stream. These are internal
operations that don't go through the turn API. No change needed.

**Tests** — `push` and `add_*` methods remain accessible as `pub(crate)` within
`jp_conversation`. Tests in other crates can use `start_turn` +
`current_turn_mut`, or a test helper.

## Drawbacks

- **Moderate refactor scope.** Every caller of `add_*` methods in `jp_cli`
  needs updating. The turn loop and coordinator are the main touchpoints.
- **Test verbosity.** Constructing test streams requires going through
  `start_turn` + `current_turn_mut` instead of raw `push`. Mitigated by keeping
  `push` as `pub(crate)`.

## Alternatives

**Do nothing.** Keep `sanitize()` as the guard. This works today but relies on
callers remembering to sanitize. The gap between "what the API allows" and "what
produces a valid stream" remains.

**Make `push` validate invariants.** Each `push` call checks the stream state
and rejects invalid sequences. Rejected because it moves runtime checks into a
hot path and makes the error handling awkward — `push` would need to return
`Result`, changing every call site anyway.

**Builder that buffers a complete turn.** `start_turn` returns a builder that
collects events and flushes on drop. Rejected because the turn loop needs to
read intermediate stream state during a turn (e.g., `build_thread` clones the
stream mid-turn for each LLM cycle). A buffering builder would hide in-progress
events.

## Non-Goals

- Enforcing event ordering _within_ a turn (e.g., ChatResponse must follow
  ChatRequest). The turn loop's state machine handles this; the stream doesn't
  need to duplicate that logic.
- Removing `sanitize()`. It remains necessary for repairing streams loaded from
  disk or produced by fork filtering.

## Risks and Open Questions

- **What should `build()` validate?** At minimum: the turn contains at least
  one `ChatRequest`. Should it also check for orphaned tool call pairs, or
  leave that to `sanitize()`?
- **Should `start_turn` take ownership of `ChatRequest` or accept
  `impl Into<ChatRequest>`?** The current `add_chat_request` uses
  `impl Into<ChatRequest>`. Consistency suggests the same pattern.
- **Config delta handling.** `push_with_config_delta` currently exists as a
  public method. It should likely move to `TurnMut` or become `pub(crate)`.
- **Forgetting to call `build`.** `#[must_use]` on `TurnMut` warns at compile
  time if the handle is dropped without calling `build()`. But it's a warning,
  not an error. Is this sufficient, or do we need a lint-level enforcement?

## Implementation Plan

### Phase 1: Add the new API alongside the old one

Add `start_turn` and `current_turn_mut` / `TurnMut` to `ConversationStream`.
Keep existing `add_*` methods as `pub`. Migrate `TurnCoordinator` to use the
new API. This can be merged independently.

### Phase 2: Migrate remaining callers

Update the turn loop, interrupt handling, and tool coordinator to use
`current_turn_mut`. Update tests. This is mechanical but touches many files.

### Phase 3: Restrict visibility

Change `add_*` and `push` to `pub(crate)`. This is the breaking change that
enforces the invariant. Any external code still using the old API will fail to
compile.

## References

- `crates/jp_conversation/src/stream.rs` — `ConversationStream`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs` — `TurnCoordinator`
- `crates/jp_cli/src/cmd/query/turn_loop.rs` — turn loop
- `crates/jp_conversation/src/event/turn.rs` — `TurnStart`
