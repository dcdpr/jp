# RFD 138: Clickable Tool Call Headers

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-09
- **Extends**: [RFD 097]
- **Summary**: Adds a per-tool `header_link` template that turns the tool name
  in a tool call header into an OSC 8 hyperlink.

## Summary

A new per-tool style option, `conversation.tools.<tool>.style.header_link`,
turns the tool name in the `Calling tool …` header into an OSC 8 hyperlink.
Its value is a link template where JP substitutes `{{workspace_id}}`,
`{{conversation_id}}`, and `{{event_id}}` and emits the resulting URI.
JP writes no files and attaches no meaning to the URI scheme; the user's
terminal and click handler own resolution.
To support handlers that read stored conversation files, `jp conversation path`
gains a `--loaded` flag that prints the path to the conversation copy JP itself
loads.

## Motivation

Tool call chrome is compact by design: arguments are previewed, results are
truncated or hidden, and the full exchange scrolls off screen.
Users who want to inspect what a tool call sent and received (to debug a failing
tool or audit an assistant decision) have no direct path from the rendered
header to the underlying event data.

That data already exists on disk.
JP persists every tool call request and response in `events.json`, and every
stream entry carries a stable Event ID (W-00dxybc#event-id, [RFD 097]).
A URI carrying `(workspace_id, conversation_id, event_id)` provides a durable
reference into `events.json`, and because conversations live in user-local
storage ([RFD 031]), that reference outlives any single checkout.
Resolving the URI at click time through a user-controlled script reads the
conversation as currently stored, requires no snapshot files, and lets the user
choose how to open the entry (for example, pretty-printing in a terminal split,
piping through `jq`, or opening an editor).

JP already emits terminal links that delegate handling to the user's
environment: `jp conversation ls` links each conversation ID to
`jp://show-metadata/<id>` and each message count to `jp://show-events/<id>`.
This RFD applies the same model to tool call headers, using a user-configured
URI template.

## Design

### User-Facing Behavior

```
Calling tool fs_read_file(path: "crates/jp_cli/src/render/tool.rs", …)
             ─────┬──────
                  └─ OSC 8 hyperlink → my_script://a1b2c/jp-c17457886043/k3m9x2a
```

Configuration:

```toml
[conversation.tools.fs_read_file.style]
header_link = "my_script://{{workspace_id}}/{{conversation_id}}/{{event_id}}"
```

When `header_link` is set, JP wraps the tool name in the permanent tool call
header in an OSC 8 hyperlink targeting the expanded template.
When unset (the default), the header renders without a hyperlink.

Because `header_link` belongs to the per-tool `style` block, it can be set for
individual tools or as a wildcard default under
`[conversation.tools.'*'.style]`.
Per-tool style settings override wildcard defaults field by field, so a wildcard
`header_link` applies to every tool unless overridden, even when a tool
customizes other `style` fields.
Setting `header_link = false` disables the link for a specific tool:

```toml
[conversation.tools.'*'.style]
header_link = "my_script://{{workspace_id}}/{{conversation_id}}/{{event_id}}"

[conversation.tools.describe_tools.style]
header_link = false
```

`header_link = true` is rejected because JP has no built-in default template.

Matching `jp conversation ls`, JP emits the OSC 8 escape sequence only when
pretty-printing is enabled; plain output (such as a redirected stream) renders
the bare tool name.

### Division of Responsibility

JP's responsibility ends at emitting the OSC 8 escape sequence:

1. **JP writes no files.** The hyperlink is an identifier reference, not a file
   path.
2. **The URI scheme is opaque to JP.** `my_script://` in the examples is
   arbitrary.
   Users configure their terminal click hook (such as WezTerm's `open-uri`
   event) or OS scheme handler to act on the link.
   JP neither registers handlers nor verifies that one is installed.

An example click-handler script:

```sh
#!/usr/bin/env sh
# Invoked with: workspace_id conversation_id event_id
path=
while IFS= read -r root; do
  [ -n "$root" ] || continue
  path=$(jp -w "$root" conversation path --events --loaded "$2" 2>/dev/null) &&
    break
done <<EOF
$(jp w show "$1" --format json | jq -r '.checkouts[].path')
EOF

if [ -z "$path" ]; then
  echo "no live checkout of workspace $1 holds conversation $2" >&2
  exit 1
fi

jq -e --arg id "$3" '
  .[]
  | select(.event_id == $id)
  | .arguments |= walk(if type == "string" then @base64d else . end)
' "$path" || {
  echo "no entry with Event ID $3 in $path" >&2
  exit 1
}
```

`jp w show` lists the workspace's live checkouts in most-recently-used order
([RFD 087]), and the script queries each checkout until one resolves the
conversation.
`jp conversation path --events --loaded` prints the path to the `events.json`
file that JP loads from that checkout (see [Resolving the Loaded Conversation
Copy](#resolving-the-loaded-conversation-copy)).
If a checkout cannot resolve the conversation (which happens when a conversation
exists only in another checkout's workspace projection), the loop continues to
the next checkout.
Reading checkout paths line by line preserves paths that contain spaces.

The script exits with an error if no live checkout holds the conversation or if
`jq -e` finds no entry matching the Event ID.

In `events.json`, the handler selects the `tool_call_request` entry by
`event_id` and can pair it with the `tool_call_response` entry in the same turn
that shares the same tool call `id`.
Because stored tool arguments and response content are base64-encoded
(`jp_conversation::storage`), the handler decodes them (as the `walk` filter
above does for `.arguments`).
If the user clicks the link while the tool is still executing, `events.json`
contains only the request; clicking again after completion finds both request
and response.

### Resolving the Loaded Conversation Copy

A projected conversation exists in two places: in user-local storage and in the
checkout's `.jp/conversations/` workspace projection ([RFD 031]).
When loading a conversation, JP selects whichever copy has the newer stream
files (`base_config.json` and `events.json`), preferring the user-local copy on
a tie.
By contrast, `jp conversation path` always returns the workspace projection path
for projected conversations so users can locate the version-controlled files
([RFD 047]).

For a click handler reading conversation data, the workspace projection path is
wrong whenever that checkout's projection is older than user-local storage (for
example, after the conversation was continued from another checkout).
Reading the stale projection shows a completed tool call as unanswered or misses
the entry entirely.

To expose JP's load resolution rule, `jp conversation path` gains a `--loaded`
flag:

```sh
$ jp conversation path --events jp-c17457886043
/home/user/project/.jp/conversations/17457886043-fix-the-parser/events.json

$ jp conversation path --events --loaded jp-c17457886043
/home/user/.local/share/jp/workspace/project-a1b2c/conversations/17457886043-fix-the-parser/events.json
```

- With `--events` or `--base-config`, `--loaded` prints the file from whichever
  copy has the newer stream (both flags always resolve to the same copy).
- With `--metadata`, `--loaded` prints `metadata.json` from whichever copy has
  the newer metadata file, matching JP's independent metadata load rule.
- Without a file flag (`--events`, `--base-config`, or `--metadata`), `--loaded`
  returns a usage error because a conversation's stream and metadata can load
  from different directories.

Without `--loaded`, `jp conversation path` behaves as before.
`jp conversation edit` continues to open the workspace projection (which it
syncs back to user-local storage after editing), and existing scripts that rely
on the default path output are unaffected.

### Placeholders

| Placeholder           | Value                                               |
| --------------------- | --------------------------------------------------- |
| `{{workspace_id}}`    | ID of the workspace the conversation belongs to     |
| `{{conversation_id}}` | ID of the conversation being rendered               |
| `{{event_id}}`        | Event ID of the tool call's `ToolCallRequest` entry |

JP percent-encodes placeholder values before substitution.
Generated IDs contain only unreserved characters and are unchanged by encoding,
but hand-edited Event IDs can contain arbitrary non-empty strings.
A template may use any subset of the three placeholders, including none.
Unrecognized placeholders and empty template strings fail configuration
validation alongside the other `conversation.tools` checks (use `false` to
disable the link).

`header_link` is a literal placeholder string rather than an [RFC 6570] URI
Template or a full template language: JP replaces only the three placeholders
above via exact-match substitution.
Although the `{{placeholder}}` syntax matches JP's MiniJinja query and
tool-command templates, `header_link` does not invoke MiniJinja.

Because the template comes from configuration and is embedded inside an OSC 8
escape sequence, `jp_term::osc::hyperlink` strips all control characters from
the expanded target before emitting it so a template cannot terminate the escape
sequence early.

### Link Scope and Replay Behavior

JP links only the **permanent** tool call header: the `Calling tool …` line
rendered after the `ToolCallRequest` is committed to the conversation stream and
assigned an Event ID.
The transient preparing row shown while tool arguments stream in is not linked:
it renders before the request is committed (when no Event ID exists) and is
erased as soon as the permanent header replaces it.

Every permanent header for a tool call receives the link regardless of how the
header is formatted: built-in parameter styles, custom formatter output, and the
per-operation headers of a fanned-out tool call ([RFD 117], which all link to
their shared `ToolCallRequest` entry).
Tool calls that render no header (because the tool is hidden or tool chrome is
disabled) emit no link.

Replaying a conversation with `jp conversation print` renders the same links
from the persisted Event IDs, making historical transcripts clickable.
During replay, `header_link` resolves from the current invocation's
configuration rather than the historical turn configuration.
This is a deliberate exception to two standard replay rules:

- **Per-turn configuration.** Replay normally renders each turn using the
  configuration snapshot stored with that turn.
  However, a link target is terminal integration metadata rather than visual
  styling: it works only if the current terminal environment has a handler for
  it.
  Resolving `header_link` from stored turn configs would emit dead links for
  removed handlers and leave older transcripts unlinked after a user configures
  a new handler.
- **`--style` presets.** A `--style` preset replaces each tool's `style` block
  to control visible chrome, but it does not clear `header_link`.
  Whenever a preset leaves a permanent tool call header visible, the configured
  `header_link` still applies.

Because the link addresses a stream entry rather than a temporary file, it
cannot break due to temp-file cleanup.
Resolution can still fail at click time if:

- the stream entry was deleted or its `event_id` was edited by hand;
- the conversation was deleted, or the turn ran with `--no-persist` so its
  events were never written to disk;
- the workspace has no remaining live checkouts; or
- the conversation predates Event IDs and the random IDs assigned on load have
  not yet been persisted.

Reporting a failed resolution is the click handler's responsibility.

## Drawbacks

- **Requires external setup.** `header_link` is unset by default, and setting it
  is useful only after the user configures a terminal click hook or OS scheme
  handler.
  Providing a working default out of the box would require JP to write and
  garbage-collect per-call snapshot files with a stable schema.
- **Handlers depend on the on-disk event format.** A click handler must locate
  entries in `events.json` and base64-decode their fields itself.
  Until JP provides a CLI command to print a single stream entry by Event ID,
  handlers duplicate that decoding logic and couple to the storage format.
- **Clicks during tool execution see only the request.** Clicking a header
  before the tool finishes shows the `ToolCallRequest` without a matching
  `ToolCallResponse`.

## Alternatives

- **Write per-call JSON files behind `file://` links.** JP could write a file
  per tool call containing the request (and later the response) and link the
  header to that file.
  Rejected because it duplicates `events.json` into a second user-facing schema
  that must be versioned, requires amending files in place as responses arrive,
  needs garbage collection, and can still serve stale data between write and
  click.
- **Resolve `jp://…?events=<event_id>` URIs through the attachment system.** An
  Event ID selector on `jp://` attachments may be useful in its own right, but
  clickable headers do not need it because the click handler reads `events.json`
  directly.
  Deferred until an attachment use case requires it.
- **Default `header_link` to a `file://` URL pointing at `events.json`.**
  Opening the entire raw event stream (with base64-encoded payloads) in the OS
  default JSON viewer is worse than having no link.
- **Link the transient preparing row.** Linking the preparing row would require
  reserving an Event ID on `ConversationStream` before committing the
  `ToolCallRequest`.
  Because the preparing row appears only briefly before the permanent header
  replaces it in scrollback, the extra stream API is not worth the complexity.
- **Change `jp conversation path --events` to print the loaded copy by
  default.** Rejected because `jp conversation edit` shares the default path
  resolution and must open the workspace projection, and changing the default
  would break existing scripts that expect the workspace path for projected
  conversations.
- **Let the click handler compare modification times itself.** Without
  `--loaded`, `jp conversation path` cannot print the user-local path of a
  projected conversation, so every handler would have to reconstruct the
  User-Workspace Directory layout and reimplement JP's stream-loading rule.

## Non-Goals

- Writing, storing, or garbage-collecting snapshot files.
- Changing `results_file_link`, inline result truncation, tool chrome layout, or
  request/response pairing.
- Registering OS URI scheme handlers or bundling click-handler scripts.
- Adding a CLI command that prints a decoded stream entry by Event ID (worth
  proposing in a separate RFD).
- Changing which conversation copy `jp conversation edit` opens; overwriting a
  newer user-local copy from a stale workspace projection is tracked separately
  in [T-1nf8vzp].

## Risks and Open Questions

- **Interaction with bounded tool output ([RFD 115]).** RFD 115 (Accepted)
  truncates large tool responses before persisting them to `events.json`.
  Once it lands, a click handler reading `events.json` sees the truncated
  response sent to the assistant rather than the tool's full output (which
  remains behind `results_file_link`).
- **Pairing requests and responses in handlers.** Handlers pair a
  `ToolCallRequest` with its `ToolCallResponse` by matching the payload tool
  call `id` within the same turn.
  Request-response event linking ([RFD 098], in Discussion) narrows candidates
  to a single answer group while preserving the same matching rule, making it a
  convenience rather than a prerequisite.
- **No pre-flight validation of scheme handlers.** JP cannot verify whether the
  terminal or OS handles the configured URI scheme, so a misconfigured scheme or
  target fails at click time.
  Configuration validation checks only placeholder names and non-empty
  templates.
- **Unimported workspace-only conversations.** A conversation that exists only
  in one checkout's `.jp/conversations/` directory and has never been imported
  into user-local storage resolves only when queried from that checkout.
  Iterating over live checkouts via `jp w show` finds it as long as that
  checkout is registered.
- **Divergent checkouts.** `jp conversation path --loaded` resolves the copy JP
  would load when run from the queried checkout.
  If checkout A's workspace projection was modified outside JP (for example, by
  `git pull`) so its `mtime` is newer than user-local storage, checkout A loads
  its projection while checkout B loads the user-local copy.
  Querying checkouts in `jp w show` order reads whichever copy the most recently
  active checkout would load.

## Implementation Plan

1. **Configuration and validation.** Add `header_link` to `DisplayStyleConfig`
   in `jp_config` with assignment, delta, fill, and partial-config support, and
   validate placeholders in `ToolsConfig`.
   Model the value as a link template or `false` (accepting a string or boolean
   in the schema, matching `results_file_link`).
   This phase can merge independently with no behavioral change when unset.
2. **Live rendering path.** Thread the committed `ToolCallRequest`'s Event ID
   from the turn coordinator to the header renderer.
   Record the `EventId` returned when pushing `ToolCallRequest` (and read it
   from existing stream entries on the turn-restart path), pass the expanded
   link target through the tool coordinator to `ToolRenderer`, and wrap the tool
   name in `render_tool_call` and `render_custom_result` with
   `jp_term::osc::hyperlink`.
   Depends on Phase 1.
3. **Replay parity.** In `TurnRenderer`, resolve per-tool `header_link`
   templates once from the current invocation's config and preserve them across
   per-turn config rebuilds and `--style` presets (see [Link Scope and Replay
   Behavior](#link-scope-and-replay-behavior)).
   Use the persisted `EventId` on each iterated entry to emit the hyperlink.
   Depends on Phase 2.
4. **`jp conversation path --loaded`.** Add `--loaded` to `jp conversation
   path`.
   Resolve `--events` and `--base-config` using the loader's stream-directory
   selection rule (`Storage::stream_dir` in `jp_storage`), and resolve
   `--metadata` using its metadata selection rule.
   Test with projected conversations where the workspace projection is older
   than user-local storage and where it is newer.
   Independent of Phases 1-3 and can merge on its own.
5. **Documentation.** Document `header_link`, `jp conversation path --loaded`,
   and complete click-handler examples (a WezTerm `open-uri` hook and an OS
   scheme handler using the `jp w show` checkout loop).
   Update the Query Stream Pipeline wiki page (W-00dxyb6) to cover header links.
   Depends on Phases 3 and 4.

## References

- [RFD 097]: Stable stream-entry Event IDs used in `{{event_id}}`.
- [RFD 087]: Out-of-directory `--workspace` targeting and checkout enumeration
  (`jp w show`) used by click handlers.
- [RFD 031]: Durable conversation storage and load-time copy selection.
- [RFD 047]: `jp conversation path`, extended here with `--loaded`.
- [RFD 096]: Terminal output sanitization (`jp_term::osc::hyperlink` strips
  control characters via `strip_controls`).
- [RFD 117]: Transparent tool call fan-out, which renders multiple headers per
  `ToolCallRequest`.
- [RFD 115]: Bounded tool output, which truncates responses stored in
  `events.json`.
- [RFD 098]: Request-response event linking, which simplifies handler-side
  request/response pairing.
- `crates/jp_cli/src/render/tool.rs`: Tool call header rendering
  (`render_tool_call` and `render_custom_result`).
- `crates/jp_cli/src/render/turn.rs`: Conversation replay, per-turn config
  rebuilds, and `--style` presets.
- `crates/jp_cli/src/cmd/query/turn_loop.rs`: Passing committed
  `ToolCallRequest` entries to the tool coordinator.
- `crates/jp_cli/src/cmd/conversation/ls.rs`: Existing `jp://` OSC 8 links.
- `crates/jp_cli/src/cmd/conversation/path.rs`: `jp conversation path`.
- `crates/jp_storage/src/load.rs`: Stream and metadata copy selection on load.
- `crates/jp_term/src/osc.rs`: OSC 8 hyperlink formatting.
- `crates/jp_config/src/conversation/tool/style.rs`: Per-tool display style
  configuration.

[RFC 6570]: https://datatracker.ietf.org/doc/html/rfc6570
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 097]: 097-stable-event-identifiers.md
[RFD 098]: 098-request-response-event-linking.md
[RFD 115]: 115-bounded-tool-output.md
[RFD 117]: 117-transparent-tool-call-fan-out.md
[T-1nf8vzp]: ../ticket/1nf8vzp-editing-a-stale-projection-overwrites-the-user-local-copy.md
