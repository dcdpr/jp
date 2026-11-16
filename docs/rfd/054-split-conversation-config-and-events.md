# RFD 054: Split Conversation Config and Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-11
- **Extended by**: [RFD 106]
- **Summary**: Extracts a conversation's initial configuration snapshot from
  `events.json` into a separate `base_config.json` file.

## Summary

This RFD moves a conversation's initial configuration snapshot from
`events.json` into `base_config.json`, leaving `events.json` with only the
conversation event stream.

## Motivation

Each conversation is stored on disk as a directory containing `metadata.json`
and `events.json`.
`events.json` is a JSON array whose first element is a `ConfigDelta` containing
the full `PartialAppConfig` from when the conversation was created.
This base config spans hundreds or thousands of lines of JSON (provider
settings, style config, tool definitions, editor preferences) before any
conversation events appear.

Packing the base config into `events.json` gets in the way when inspecting
conversations:

- Opening `events.json` in an editor requires scrolling past the config to reach
  the events.
- `grep` and `jq` queries against `events.json` match config fields.

The base config is written once when the conversation is created and never
changes; later mutations are recorded as `ConfigDelta` events in the stream.
Storing it in a separate file separates initial state from the event stream.

## Design

### Directory layout

The per-conversation directory changes from:

```txt
conversations/{id}/
  metadata.json
  events.json        # [base_config_delta, ...events...]
```

To:

```txt
conversations/{id}/
  metadata.json
  base_config.json   # PartialAppConfig snapshot
  events.json        # [event, event, ...]
```

The file is named `base_config.json` rather than `config.json` because it holds
the initial configuration snapshot, not the active config.
The active config is produced by converting the `PartialAppConfig` in
`base_config.json` to an `AppConfig` and merging every `ConfigDelta` event from
the stream on top.

### `base_config.json`

`base_config.json` stores the initial `PartialAppConfig` directly as the
top-level JSON object, with no wrapper or nesting.

Because the base config is a state snapshot rather than an event,
`base_config.json` carries no timestamp.
The conversation's creation time is already encoded at decisecond precision in
the `ConversationId`, which is the canonical source for
`ConversationStream.created_at`.

### `events.json`

`events.json` contains only `InternalEvent` entries (`ConfigDelta` mutations and
`ConversationEvent`s), with no synthetic leading element.

### `ConversationStream` serde changes

The custom `Serialize` and `Deserialize` implementations on
`ConversationStream`, which pack and unpack the base config as the first array
element, are removed.
Instead, `ConversationStream` exposes two methods for the storage layer to read
and write the split files:

```rust
impl ConversationStream {
    /// Construct a stream from a serialized base config and serialized events.
    ///
    /// The storage layer reads `base_config.json` as a `PartialAppConfig`
    /// and `events.json` as raw JSON values. Deserialization of
    /// `InternalEvent` stays inside `jp_conversation`.
    ///
    /// The returned stream has `created_at` set to `Utc::now()`. The
    /// caller should chain `.with_created_at(id.timestamp())` to set
    /// the correct creation time.
    pub fn from_stored(
        base_config: PartialAppConfig,
        events_json: Vec<serde_json::Value>,
    ) -> Result<Self, StreamError>;

    /// Decompose the stream into its storable parts.
    ///
    /// Returns the base config and the serialized events array.
    /// The storage layer writes these to `base_config.json` and
    /// `events.json` respectively.
    pub fn into_stored(self) -> Result<(PartialAppConfig, Vec<serde_json::Value>), StreamError>;
}
```

The storage layer passes raw `serde_json::Value` entries in and out without
knowing the internal event representation.
`InternalEvent` changes from `pub` to `pub(crate)`, as its usage is confined to
`stream.rs`, `turn_mut.rs`, `storage.rs`, and tests within `jp_conversation`.

### Storage layer changes

`jp_storage` defines `const BASE_CONFIG_FILE: &str = "base_config.json";` and
updates the load and persist paths:

- `load_conversation_stream` reads `base_config.json` and `events.json`
  separately and passes both to `ConversationStream::from_stored`.
  If `base_config.json` is missing, it falls back to the legacy format (see
  [Backward compatibility](#backward-compatibility)).
  If both formats fail, loading returns an error rather than falling back to the
  startup config, which would mask data corruption.
- `persist_conversations_and_events` writes `base_config.json` only when the
  file does not yet exist on disk.
  Because the base config is immutable after creation, subsequent persists skip
  it, preserving any manual edits to `base_config.json`.

### Backward compatibility

Older conversations store the base config as the first element of `events.json`
and have no `base_config.json` file.
`load_conversation_stream` migrates them transparently:

1. If `base_config.json` exists, load it and `events.json` using the split
   format.
2. If `base_config.json` is missing and the first element of `events.json` is a
   `ConfigDelta`, treat the file as the legacy format: extract the
   `PartialAppConfig` from the first element's `delta` field (discarding the
   timestamp) and pass the remaining elements as events.
3. On the next persist, write `base_config.json` and overwrite `events.json`
   without the leading config element.
4. If neither format matches, return a `LoadError` because the conversation data
   is corrupt or incomplete.

This migration path is isolated in `load_conversation_stream` and can be removed
in a future release.

### Concurrent access

[RFD 020] introduces per-conversation exclusive file locks for write operations.
The persist path writes `metadata.json`, `base_config.json`, and `events.json`
while holding the `ConversationLock`, protecting the three-file write against
concurrent mutation in the same way as the two-file write.

Read-only operations (`conversation show`, `conversation ls`) do not acquire
locks and can observe a partially written state.
This risk already exists between `metadata.json` and `events.json` and is
unchanged by adding a third file; readers that encounter parse errors retry or
surface the error.

## Drawbacks

- **Three files instead of two**: A conversation directory contains
  `metadata.json`, `base_config.json`, and `events.json`.
  Because `base_config.json` is written only at creation time, steady-state
  persistence still writes two files, and [RFD 020]'s conversation locks cover
  the multi-file write.

- **Backward-compatibility code**: Supporting the legacy format adds a second
  deserialization branch in `load_conversation_stream` until the migration path
  is removed.

## Alternatives

### Keep config in `events.json` under a top-level wrapper

Serializing `events.json` as `{ "config": {...}, "events": [...] }` instead of a
flat array avoids a third file, but still breaks existing conversations and
leaves the config at the top of `events.json` in editors.
Rejected because it carries the migration cost without solving the inspection
problem.

### Store config in `metadata.json`

Merging the base config into `metadata.json` avoids a third file, but
`metadata.json` holds lightweight fields (title, timestamps, flags) loaded
separately when listing conversations.
Rejected because embedding the full `PartialAppConfig` would bloat metadata
reads.

## Non-Goals

- **Changing the `ConfigDelta` mechanism**: Incremental config mutations within
  a conversation remain `ConfigDelta` events in `events.json`.
  This RFD moves only the initial base config out of the event stream.
- **Compressing or optimizing the config format**: `PartialAppConfig` is
  serialized as-is.
- **Changing `metadata.json`**: The conversation metadata file is unchanged.
- **Fixing `load_count_and_timestamp_events` accuracy**: This function counts
  all array elements in `events.json`, including `ConfigDelta` entries.
  Removing the leading base config eliminates the extra count from the initial
  snapshot, but mid-stream `ConfigDelta` events are still counted until event
  counting moves to turn-based counting.

## Implementation Plan

This is a single-phase change that can be merged in one PR.

1. **`jp_conversation`**: Remove the custom `Serialize` and `Deserialize`
   implementations on `ConversationStream`, change `InternalEvent` visibility
   from `pub` to `pub(crate)`, add `from_stored` and `into_stored`, and update
   snapshots.

2. **`jp_storage`**: Add the `BASE_CONFIG_FILE` constant, update
   `load_conversation_stream` with the split-file loader and legacy fallback,
   and update `persist_conversations_and_events` to write `base_config.json`
   only on first persist.
   `load_count_and_timestamp_events` is unchanged.

3. **Tests**: Update `jp_workspace` and `jp_cli` test helpers that write
   conversation files, along with snapshot files.

## References

- `crates/jp_conversation/src/stream.rs`: `ConversationStream` serde
  implementations
- `crates/jp_storage/src/load.rs`: `load_conversation_stream`
- `crates/jp_storage/src/lib.rs`: `persist_conversations_and_events`
- `crates/jp_workspace/src/lib.rs`: callers of `load_conversation_stream` and
  `persist_conversations_and_events`
- [RFD 020]: conversation locks that protect concurrent write access

[RFD 020]: 020-parallel-conversations.md
[RFD 106]: 106-collision-free-conversation-id-allocation.md
