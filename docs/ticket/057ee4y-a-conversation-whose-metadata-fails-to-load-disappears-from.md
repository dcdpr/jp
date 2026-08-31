# A conversation whose metadata fails to load disappears from every listing

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-20

`Workspace::conversations()` calls `ensure_all_metadata_loaded`, which logs a
`warn!` for each conversation whose metadata will not load and leaves that
conversation's cell unset.
The iterator then `filter_map`s on `cell.get()`, so the conversation is dropped
from the result and the call still succeeds.

```rust
// crates/jp_workspace/src/lib.rs
Err(error) => warn!(%id, %error, "Failed to load conversation metadata."),
```

Every consumer therefore receives a listing that looks complete and is not: `jp
conversation ls`, `jp serve web`, and `jp_workspace_conversations` in the FFI.
Nothing in the returned value says a conversation was skipped.

A plausible input is a `metadata.json` written partially during a crash, or one
mis-edited by hand — `jp conversation edit --events` actively encourages hand
editing, so this is not a hypothetical shape.

The failure is worst in the macOS app.
`jp_ffi` installs no tracing subscriber, so the `warn!` has nowhere to go: a
conversation vanishes from the sidebar with no log line, no error, and no
indication that the list is short.
In the CLI at least the warning reaches stderr under the usual configuration.

The fix belongs at the `jp_workspace` layer rather than in any one consumer, so
that all three agree about what a workspace contains.
Something along the lines of a listing that carries the per-entry failures
alongside the conversations that did load, letting each consumer decide how to
present them — the sidebar could show a row that says the conversation could
not be read, and `jp conversation ls` could print a count of what it skipped.

Found while triaging review feedback on the macOS app PR (dcdpr/jp\#1008,
comment 3815419227).
Not introduced there; the app only made an existing silent failure completely
silent.
