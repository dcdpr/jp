# RFD 054: Split Conversation Config and Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-11

## Summary

This RFD splits the conversation `events.json` file into two files:
`base_config.json` for the base configuration snapshot and `events.json` for the
event stream. The base configuration is currently packed as a synthetic first
element in the events array, making `events.json` difficult to inspect.

## Motivation

Each conversation is stored on disk as a directory containing `metadata.json`
and `events.json`. The events file is a JSON array whose first element is a
`ConfigDelta` containing the full `PartialAppConfig` at the time the
conversation was created. This base config is typically hundreds or thousands of
lines of JSON — provider settings, style config, tool definitions, editor
preferences — and it appears before any actual conversation events.

This creates a poor experience when inspecting conversations:

- Opening `events.json` in an editor requires scrolling past the config blob to
  reach actual events.
- `grep` and `jq` queries against events hit config fields.

The base config is conceptually different from events. It is written once at
conversation creation time and never changes (mutations happen through
`ConfigDelta` events in the stream). Storing it in a separate file reflects this
distinction.

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

The file is named `base_config.json` rather than `config.json` to make it clear
this is the initial configuration snapshot, not the active config. The active
config is the result of converting the `PartialAppConfig` in `base_config.json`
to an `AppConfig` and then merging all `ConfigDelta` events from the stream on
top.

### `base_config.json`

Contains a single JSON object: the serialized `PartialAppConfig` that was active
when the conversation was created. The config is the top-level object in the
file — no wrapper, no nesting.

The file contains no timestamp. Only events in `events.json` carry timestamps,
because timestamps record when something happened in the conversation. The base
config is a snapshot of state, not an event. The conversation's creation time is
already encoded in the `ConversationId` (decisecond precision), which is the
canonical source for `ConversationStream.created_at`.

### `events.json`

Contains only `InternalEvent` entries: `ConfigDelta` mutations and
`ConversationEvent`s. No synthetic leading element.

### `ConversationStream` serde changes

The custom `Serialize` and `Deserialize` implementations on `ConversationStream`
currently pack and unpack the base config as the first array element. These
impls are removed entirely. Instead, the storage layer serializes and
deserializes the config and events separately.

`InternalEvent` is changed from `pub` to `pub(crate)`. No code outside
`jp_conversation` references it — usage is confined to `stream.rs`,
`turn_mut.rs`, `storage.rs`, and tests within the crate.

`ConversationStream` gains two new methods for the storage layer to interact
with the split files:

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

This keeps `InternalEvent` private to the crate. The storage layer passes raw
JSON values in and gets raw JSON values out, without needing to know about the
internal event representation.

### Storage layer changes

In `jp_storage`:

- Add `const BASE_CONFIG_FILE: &str = "base_config.json";`.
- `load_conversation_stream` reads `base_config.json` and `events.json`
  separately, passing both to `ConversationStream::from_stored`. If
  `base_config.json` is missing, it falls back to the old-format migration (see
  [Backward compatibility](#backward-compatibility)). If both the new and old
  formats fail, the load returns an error — there is no silent fallback to the
  startup config, since that would mask data corruption.
- `persist_conversations_and_events`: writes `base_config.json` only when the
  conversation is first created (i.e., when `base_config.json` does not yet
  exist on disk). Since the base config is immutable after creation, subsequent
  persists skip this file. Users who manually edit `base_config.json` will see
  their changes preserved.

### Backward compatibility

Old conversations have `events.json` with the base config packed as the first
element and no `base_config.json` file. The migration is handled transparently
in `load_conversation_stream`:

1. If `base_config.json` exists, load it and `events.json` using the new format.
2. If `base_config.json` is missing but `events.json` exists and its first
   element is a `ConfigDelta`, treat it as the old format: extract the
   `PartialAppConfig` from the first element's `delta` field (discarding the
   timestamp) and use the remainder as events.
3. On next persist, write `base_config.json` (since it doesn't exist yet) and
   overwrite `events.json` without the leading config element.
4. If neither format is detected, return a `LoadError` — the conversation data
   is corrupt or incomplete.

This migration layer is isolated in `load_conversation_stream` and can be
removed in a future release once enough time has passed.

### Concurrent access

[RFD 020] introduces per-conversation exclusive file locks for write operations.
The persist path — which writes `metadata.json`, `base_config.json`, and
`events.json` — runs under the `ConversationLock`. This means the three-file
write is already protected against concurrent mutation by the same mechanism
that protects the current two-file write.

Read-only operations (`conversation show`, `conversation ls`) do not acquire
locks and could observe a partially-written state. This risk already exists
between `metadata.json` and `events.json` and is unchanged by adding a third
file. Readers that encounter parse errors retry or surface the error, which is
acceptable.

## Drawbacks

- **Three files instead of two**: Persisting a conversation now involves three
  files (`metadata.json`, `base_config.json`, `events.json`) instead of two.
  Since `base_config.json` is only written once at creation time, the steady-
  state cost is the same as today (two files per persist). The window for
  inconsistency between the files is addressed by [RFD 020]'s conversation
  locks.

- **Backward-compatibility code**: The migration layer adds complexity to the
  load path. It is isolated and removable, but until it is removed, there are
  two deserialization paths to maintain.

## Alternatives

### Keep config in `events.json` but move it to a top-level wrapper

Serialize as `{ "config": {...}, "events": [...] }` instead of a flat array.
This avoids a second file but changes the format in a way that still breaks
existing conversations and doesn't solve the "scroll past config" problem in
editors (the config is still at the top of the same file). Rejected because it
has most of the costs of the proposed approach without the ergonomic benefit.

### Store config in `metadata.json`

Merge the base config into the existing metadata file. This avoids a third file,
but `metadata.json` currently contains lightweight conversation metadata (title,
timestamps, flags) and is loaded separately from events for listing purposes.
Embedding the full config would bloat metadata loading. Rejected.

## Non-Goals

- **Changing the `ConfigDelta` mechanism**: Incremental config mutations within
  a conversation remain as `ConfigDelta` events in `events.json`. This RFD only
  moves the _base_ config out of the event stream.
- **Compressing or optimizing the config format**: The `PartialAppConfig` is
  serialized as-is. Future work could investigate other forms of compression or
  optimization, but that is out of scope.
- **Changing `metadata.json`**: The conversation metadata file is unchanged.
- **Fixing `load_count_and_timestamp_events` accuracy**: The function currently
  counts all array elements including `ConfigDelta` entries. Removing the
  leading base config improves accuracy by one, but mid-stream `ConfigDelta`
  events are still counted. A future change to turn-based counting will address
  this properly.

## Implementation Plan

This is a single-phase change that can be merged in one PR.

1. **`jp_conversation`**: Remove custom `Serialize`/`Deserialize` impls on
   `ConversationStream`. Change `InternalEvent` visibility from `pub` to
   `pub(crate)`. Add `from_stored`/`into_stored` methods. Update snapshots.

2. **`jp_storage`**: Add `BASE_CONFIG_FILE` constant. Update
   `load_conversation_stream` with new-format loading and old-format migration
   fallback. Update `persist_conversations_and_events` to write
   `base_config.json` on first persist only. No changes to
   `load_count_and_timestamp_events`.

3. **Tests**: Update `jp_workspace` and `jp_cli` test helpers that write
   conversation files. Update snapshot files.

## References

- `crates/jp_conversation/src/stream.rs` — `ConversationStream` serde impls
- `crates/jp_storage/src/load.rs` — `load_conversation_stream`
- `crates/jp_storage/src/lib.rs` — `persist_conversations_and_events`
- `crates/jp_workspace/src/lib.rs` — callers of `load_conversation_stream` and
  `persist_conversations_and_events`
- [RFD 020] — conversation locks that protect concurrent write access

[RFD 020]: 020-parallel-conversations.md
