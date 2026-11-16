# RFD 006: Turn-Scoped Stream Mutations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-18
- **Summary**: Restricts `ConversationStream` mutations to turn-scoped accessors
  so invalid event sequences cannot be constructed through the public API.

## Summary

This RFD restricts public `ConversationStream` mutations to turn-scoped
accessors (`start_turn` and `current_turn_mut`) so callers cannot construct a
stream without valid turn boundaries.

## Motivation

`ConversationStream` exposes raw `push`, `add_chat_request`,
`add_chat_response`, and similar methods that let callers append events in any
order.
Nothing prevents a caller from creating a stream without a `TurnStart`, adding a
`ChatResponse` before a `ChatRequest`, or pushing events outside a turn
boundary.

This has caused bugs in practice:

- Forking with `--from`/`--until` can produce a stream that starts with an
  assistant response, which fails on providers like Anthropic that require the
  first message to come from the user.
  Fork's range flags (`--from`/`--to`) resolve to whole turns, so range
  filtering no longer produces this shape, but the invariant still matters for
  every other caller.
- Streams without `TurnStart` markers break `--last`, which counts turn
  boundaries.

`sanitize()` repairs these invariant violations after the fact, but it is a
caller-side patch: callers must remember to invoke it, and the raw mutation
methods remain open to misuse.

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

`current_turn_mut()` is infallible: if the stream has no turns yet, it injects
an empty `TurnStart` and returns a handle to it.
This avoids forcing every call site to handle `None` or `Err` for a case that
should not occur in practice.

### `TurnMut<'_>`

`TurnMut` wraps `&mut ConversationStream` and buffers events until `build()`
validates, sanitizes, and flushes them to the stream.
Partial or invalid event sequences never appear on the stream itself.

Because `TurnMut` is short-lived, this model fits the turn loop: callers acquire
a handle, append events, and call `build()` to release the borrow.
Code that reads the stream (such as `build_thread` cloning the stream for each
LLM cycle) always runs between `build()` calls, never while a `TurnMut` is held.

`TurnMut` provides two chaining styles and a terminal `build()` method:

- **`with_xxx(&mut self, x: X) -> &mut TurnMut`**: Borrowed receiver for
  mutating an existing binding.
- **`add_xxx(mut self, x: X) -> TurnMut`**: Owned receiver for fluent builder
  chains.
- **`build(self) -> Result<()>`**: Validates, sanitizes, and flushes the
  buffered events, releasing the mutable borrow.

All methods are `#[must_use]` to prevent silently dropping a `TurnMut` without
calling `build()`.
There is no custom `Drop` implementation.

`TurnMut` exposes the following methods:

- `add_chat_request` / `with_chat_request`: Appends an interrupt reply within an
  existing turn.
- `add_chat_response` / `with_chat_response`
- `add_tool_call_request` / `with_tool_call_request`
- `add_tool_call_response` / `with_tool_call_response`
- `add_inquiry_request` / `with_inquiry_request`
- `add_inquiry_response` / `with_inquiry_response`
- `build`: Validates and commits the buffered events.

It omits two `ConversationStream` methods:

- `add_turn_start`: Only `start_turn()` can create turn boundaries.
- `push`: Raw event insertion is restricted to crate-internal code.

### Visibility Changes

| Method                 | Current   | Proposed          |
| ---------------------- | --------- | ----------------- |
| `start_turn`           | N/A (new) | `pub`             |
| `current_turn_mut`     | N/A (new) | `pub`             |
| `push`                 | `pub`     | `pub(crate)`      |
| `add_chat_request`     | `pub`     | `pub(crate)`      |
| `add_chat_response`    | `pub`     | `pub(crate)`      |
| `add_turn_start`       | `pub`     | `pub(crate)`      |
| `add_tool_call_*`      | `pub`     | `pub(crate)`      |
| `add_inquiry_*`        | `pub`     | `pub(crate)`      |
| `sanitize`             | `pub`     | `pub` (unchanged) |
| `retain`, `iter`, etc. | `pub`     | `pub` (unchanged) |

Internal methods (`sanitize`, `trim_trailing_empty_turn`) continue to mutate
`self.events` directly; `TurnMut` guards the public API rather than internal
repair logic.

### Caller Migration

- **`TurnCoordinator::start_turn`**: Replaces `add_turn_start` followed by
  `add_chat_request` with `stream.start_turn(request)`.
- **`TurnCoordinator` event handlers**: Replace direct calls to
  `stream.add_chat_response`, `stream.add_tool_call_response`, and sibling
  methods with `stream.current_turn_mut().with_chat_response(...).build()?`.
  Each handler acquires and consumes `TurnMut` within a synchronous block,
  avoiding borrow conflicts across async boundaries.
- **`InterruptAction::Reply`**: Replaces `stream.add_chat_request(...)` with
  `stream.current_turn_mut().with_chat_request(...).build()?`, since an in-turn
  interrupt reply appends a `ChatRequest` without starting a new turn.
- **Fork**: Uses `extend` and `retain` directly on the stream and requires no
  changes.
- **Tests**: `push` and `add_*` remain `pub(crate)` within `jp_conversation`.
  Tests in other crates use `start_turn` and `current_turn_mut`, or a test
  helper.

## Drawbacks

- **Moderate refactor scope**: Every `add_*` caller in `jp_cli`, primarily in
  the turn loop and `TurnCoordinator`, must be updated.
- **Test verbosity**: Constructing test streams outside `jp_conversation`
  requires `start_turn` and `current_turn_mut` instead of raw `push` calls,
  though keeping `push` as `pub(crate)` avoids this cost for unit tests within
  `jp_conversation`.

## Alternatives

- **Do nothing**: Keep `sanitize()` as the sole guard.
  Callers must still remember to invoke it, and the public API remains able to
  construct invalid streams.
- **Validate invariants in `push`**: Have each `push` call inspect stream state
  and reject invalid event sequences.
  Rejected because it adds runtime checks to a hot path and forces `push` to
  return `Result`, which still requires updating every call site.
- **Buffer a complete turn**: Have `start_turn` return a builder that collects
  events and flushes them on drop.
  Rejected because the turn loop reads intermediate stream state mid-turn (for
  example, `build_thread` clones the stream for each LLM cycle), and a buffering
  builder would hide in-progress events.

## Non-Goals

- **Enforcing event ordering within a turn**: The turn loop's state machine
  already ensures that `ChatResponse` follows `ChatRequest`, so
  `ConversationStream` does not duplicate that logic.
- **Removing `sanitize()`**: Streams loaded from disk or produced by fork
  filtering still require post-hoc repair.

## Risks and Open Questions

- **Validation scope in `build()`**: At minimum, `build()` must verify that the
  turn contains at least one `ChatRequest`.
  Should it also check for orphaned tool call pairs, or leave that to
  `sanitize()`?
- **`start_turn` parameter type**: Should `start_turn` take ownership of
  `ChatRequest` or accept `impl Into<ChatRequest>` to match `add_chat_request`?
- **Config delta handling**: `push_with_config_delta` is a public method on
  `ConversationStream` and should move to `TurnMut` or become `pub(crate)`.
- **Forgetting to call `build()`**: `#[must_use]` on `TurnMut` emits a
  compile-time warning if a handle is dropped without calling `build()`.
  Is a warning sufficient, or do we need lint-level enforcement?

## Implementation Plan

### Phase 1: Add the new API alongside the old one

Add `start_turn`, `current_turn_mut`, and `TurnMut` to `ConversationStream`
while keeping the existing `add_*` methods `pub`, and migrate `TurnCoordinator`
to the new API.
This phase can be merged independently.

### Phase 2: Migrate remaining callers

Update the turn loop, interrupt handling, tool coordinator, and tests to use
`current_turn_mut`.

### Phase 3: Restrict visibility

Change `add_*` and `push` to `pub(crate)` so external callers can no longer
bypass turn boundaries.

## References

- `crates/jp_conversation/src/stream.rs`: `ConversationStream`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs`: `TurnCoordinator`
- `crates/jp_cli/src/cmd/query/turn_loop.rs`: turn loop
- `crates/jp_conversation/src/event/turn.rs`: `TurnStart`
