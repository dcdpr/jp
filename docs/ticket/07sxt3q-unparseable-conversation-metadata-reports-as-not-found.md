# Unparseable conversation metadata reports as Not found

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-24

A conversation whose `metadata.json` exists but does not deserialize is reported
as missing:

```console
$ jp q
Not found

 target  Conversation metadata
     id  jp-c17874925101
```

The file is right there.
Nothing in the output says it could not be read, what about it could not be
read, or which file to look at.

## Where it happens

`maybe_init_conversation` (`crates/jp_workspace/src/lib.rs:896`) discards the
load error:

```rust
let Ok(meta) = loader.load_conversation_metadata(id) else {
    warn!(%id, "Failed to load conversation metadata. Skipping.");
    return;
};
```

The `OnceLock` stays empty, so `Workspace::metadata` falls through to
`Error::not_found("Conversation metadata", id)`.
The reason reaches the trace log and nowhere else.
`maybe_init_events` has the same shape.

Sanitization does not catch it first: `validate.rs` only confirms
`metadata.json` is a JSON object, using `IgnoredAny` to skip the field values
([RFD 052]).
A file whose *fields* are wrong passes validation and then fails at load, which
is exactly the gap this falls into.

## Why it matters

Every unreadable-metadata case is indistinguishable from a deleted conversation,
so the reported cause is wrong rather than merely thin:

- A hand-edited `metadata.json` with a typo — [RFD 031] supports editing these
  files directly, so this is the case a user reaches on their own.
- A partially written file from an interrupted persist.
- A field whose *type* changed between two builds.
  Found this way, running two development builds against one workspace: a build
  predating [RFD 103] read a `labels` array as a string and reported the
  conversation missing.

The type-change case does not reach released binaries — a build that predates a
field skips it as unknown — so it is a hazard for people running several builds
rather than a shipping bug.
The other two are not.

All three are recoverable in seconds *if* the error names the file and the parse
failure.
As "Not found", the natural next step is to go looking for a conversation the
user believes they lost.

## Shape of the fix

Thread the load error through the lazy-init path rather than collapsing it:
`maybe_init_conversation` and `maybe_init_events` return the failure, and
`Workspace::metadata` / `events` distinguish "no such conversation" from "the
conversation is there and unreadable", carrying the path and the underlying
serde message.

`LoadError` already holds both (`jp_storage::load`), so the information exists
and is being dropped at the `OnceLock` boundary.
The work is in the callers: the init helpers are used from several read paths
that currently cannot fail.

## Not in scope

Making metadata tolerant of *unknown* fields.
It already is — `Conversation` carries no `deny_unknown_fields`, so a field
added by a newer JP is skipped, and `an_unknown_field_is_ignored` in
`conversation_tests.rs` pins that.
It is what keeps a released binary reading conversations written by a
development build.

That tolerance does not extend to a known field whose type changed, which is the
case above: no amount of unknown-field skipping helps a reader that knows
`labels` and expects a string.
Widening the reader before narrowing the writer is what covers that, and it is a
release-sequencing question rather than something this ticket can fix.

[RFD 031]: ../rfd/031-durable-conversation-storage-with-workspace-projection.md
[RFD 052]: ../rfd/052-workspace-data-store-sanitization.md
[RFD 103]: ../rfd/103-multi-value-conversation-labels.md
