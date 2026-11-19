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
The value is a link template: JP substitutes `{{workspace_id}}`,
`{{conversation_id}}`, and `{{event_id}}`, and emits the result.
JP writes no file and attaches no meaning to the URI scheme.
The user's terminal and click handler own resolution.
For handlers that read a stored conversation, `jp conversation path` gains a
`--loaded` flag that prints the copy JP itself loads.

## Motivation

Tool chrome is compact by design: arguments are previewed, results are truncated
or hidden, and the full exchange scrolls away.
A user who wants to inspect what a tool call sent and received, to debug a
misbehaving tool or audit an assistant decision, has no path from the rendered
chrome to the data.

The data exists.
JP persists every tool call request and response in the conversation's
`events.json`, and every stream entry has an Event ID (W-00dxybc#event-id, [RFD
097]).
A link that carries `(workspace, conversation, event)` is a durable reference
into that source of truth.
Conversations live in the user-local store ([RFD 031]), so the reference
outlives any single checkout.
Resolution at click time, by a script the user controls, reads the conversation
as it is stored at that moment, needs no snapshot, and lets the user decide what
"open" means: pretty-print in a split pane, pipe through `jq`, open an editor.

JP already emits links of this kind.
`jp conversation ls` links each conversation ID to `jp://show-metadata/<id>` and
each message count to `jp://show-events/<id>`, and leaves their handling to the
terminal.
This RFD gives tool call headers the same treatment, with the link target under
the user's control.

## Design

### What the user sees

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

When `header_link` is set, the tool name in the permanent tool call header is
wrapped in an OSC 8 hyperlink whose target is the template with its placeholders
substituted.
When unset (the default), the header renders as it does today.

`header_link` is a field of the per-tool style block, so it is settable per tool
and as a default for every tool under `[conversation.tools.'*'.style]`.
A tool's own style overrides the defaults field by field, so one `header_link`
under `'*'` reaches every tool, including tools that set other style keys.
`header_link = false` disables the link, which is how a tool opts out of a `'*'`
default:

```toml
[conversation.tools.'*'.style]
header_link = "my_script://{{workspace_id}}/{{conversation_id}}/{{event_id}}"

[conversation.tools.describe_tools.style]
header_link = false
```

`true` is not accepted: JP has no default template to turn on.

The link is emitted only when pretty printing is enabled, the same condition `jp
conversation ls` applies to its links.
Plain output, such as a redirected stream, carries the tool name without the
escape sequence.

### Division of responsibility

JP's responsibility ends at emitting the escape sequence.

1. **JP writes no file.** The link is a reference, not a resource.
2. **The scheme is not special.** `my_script://` above is arbitrary.
   The user configures their environment so the link does something useful: a
   terminal click hook (for example WezTerm's `open-uri` event) or an OS-level
   scheme handler.
   JP neither registers handlers nor checks that one exists.

A sketch of a click handler:

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

`jp w show` lists the workspace's live checkouts ([RFD 087]), most recently used
first, and the handler asks each in turn.
`jp conversation path --events --loaded` prints the path to the `events.json` JP
itself loads from that checkout (see [Which copy a handler
reads](#which-copy-a-handler-reads)).
The loop moves on when a checkout cannot resolve the conversation, which happens
for a conversation that exists only in another checkout's workspace projection.
The checkout paths are read line by line, so a path with spaces stays whole.

The handler fails with a message when no checkout resolves the conversation, and
when the file holds no entry with the Event ID: `jq -e` exits non-zero when the
filter selects nothing.

`events.json` is an array of stream entries.
The handler finds the `tool_call_request` entry by its `event_id`, and pairs it
with the `tool_call_response` entry in the same turn that carries the same
payload `id`.
Stored tool call arguments and response content are base64-encoded
(`jp_conversation::storage`), so the handler decodes them, as the `walk` above
does for the arguments.
Clicked mid-execution, the response is not in the file yet; a later click sees
the completed pair.

### Which copy a handler reads

A projected conversation is stored more than once: in the user-local store, and
as a workspace projection in each checkout that holds it ([RFD 031]).
JP loads the copy whose stream files (`base_config.json` and `events.json`) were
modified last, and the user-local copy on a tie.
`jp conversation path` does not follow that rule.
For a projected conversation it prints the workspace copy, the one visible to
version control ([RFD 047]).

For a reader, that is the wrong file whenever the checkout's projection is the
older copy.
A conversation continued from one checkout leaves an older projection in
another.
A handler that reads the older projection shows a completed tool call as
unanswered, or finds no entry at all.

`jp conversation path` gains a `--loaded` flag, which prints the copy JP loads:

```sh
$ jp conversation path --events jp-c17457886043
/home/user/project/.jp/conversations/17457886043-fix-the-parser/events.json

$ jp conversation path --events --loaded jp-c17457886043
/home/user/.local/share/jp/workspace/project-a1b2c/conversations/17457886043-fix-the-parser/events.json
```

- With `--events` or `--base-config`, it prints the file from the copy with the
  newer stream, the same copy for both.
- With `--metadata`, it prints the copy with the newer `metadata.json`, which JP
  resolves separately from the stream.
- Without a file flag it is a usage error: the stream and the metadata can load
  from different copies, so no single directory is the loaded one.

The output without the flag stays as it is.
`jp conversation edit` shares the command's path resolution and opens the
workspace copy, which it copies to the user-local store after the edit, and
scripts depend on what the command prints today.

### Placeholders

| Placeholder           | Value                                               |
| --------------------- | --------------------------------------------------- |
| `{{workspace_id}}`    | ID of the workspace the conversation belongs to     |
| `{{conversation_id}}` | ID of the conversation being rendered               |
| `{{event_id}}`        | Event ID of the tool call's `ToolCallRequest` entry |

Placeholder values are percent-encoded before substitution.
For IDs JP generates, which contain only unreserved characters, the encoding
changes nothing; it exists for hand-edited Event IDs, which may be any non-empty
string.
A template may use any subset of the placeholders, including none.
An unrecognized placeholder is a configuration validation error, reported with
the other `conversation.tools` checks.
So is an empty template: `false` is the one way to write "no link".

The value is a link template, not an [RFC 6570] URI Template and not a template
language: JP replaces the three placeholders above and nothing else.
The `{{placeholder}}` spelling matches the minijinja templates JP uses for
queries and tool commands, but substitution here is exact-match replacement by a
small private replacer, not a template engine.

The template is config-supplied and ends up inside an escape sequence.
`jp_term::osc::hyperlink` removes every control character from a link target
before emitting it, so a template cannot end the sequence early.

### What is linked, and when

Only the **permanent** header is linked: the `Calling tool …` line written once
the `ToolCallRequest` is committed to the conversation stream and has an Event
ID.
The preparing row, which names tools while their arguments stream in, is not
linked.
It is drawn before the request is committed, so no Event ID exists yet, and it
is erased when the permanent header replaces it.

Every permanent header of a call carries the link, whichever path writes it: a
built-in parameter style, a formatter's output, or the per-operation headers of
a fanned-out call ([RFD 117]), which all link to the one request entry they came
from.
A call that renders no header, because the tool is hidden or tool chrome is off,
has no link.

Replay (`jp conversation print`) renders the same links from persisted Event
IDs, so older transcripts are clickable too.
During replay, `header_link` resolves from the current invocation's
configuration.
That is a deliberate exception to two replay mechanisms:

- Replay renders each turn with the configuration stored for that turn.
  A link target is terminal metadata, not visible style: it is useful only if
  the reader's environment has a handler for it.
  Per-turn resolution would emit dead links from templates since removed, and
  leave old transcripts unlinked for the user who just set up a handler.
- A `--style` preset replaces every tool's style block to control which chrome
  is visible.
  It does not clear `header_link`: whenever a preset leaves a permanent header
  on screen, the link applies.

The link cannot dangle the way a deleted temp-file path does, because it names
an entry instead of a file.
It can still fail to resolve:

- the entry was deleted, or its ID edited by hand;
- the conversation was removed, or the turn ran under `--no-persist` and its
  events were never stored;
- the workspace has no live checkout left;
- the conversation predates Event IDs, and the IDs assigned when it was loaded
  have not been saved yet.

Surfacing a failed resolution is the handler's job.

## Drawbacks

- **No behavior out of the box.** An unset `header_link` does nothing, and a set
  one is only useful after the user wires up a handler.
  The alternative is JP owning files, a schema, and garbage collection so that a
  default link lands somewhere.
- **Handlers read a storage format.** A handler locates an entry in
  `events.json` and decodes its base64 fields itself.
  JP has no command that prints one entry by Event ID, so every handler repeats
  that work and depends on the stored layout.
- **Mid-execution clicks race the response.** The handler sees a request without
  a response until the call completes.

## Alternatives

- **Per-call JSON files behind `file://` links.** JP writes a file per tool call
  holding the request and, later, the response, and links the header to it.
  Rejected: it duplicates `events.json` into a second user-visible schema that
  needs versioning, adds an amend-in-place write model and garbage collection,
  and still serves stale data between write and click.
- **A `jp://…?events=<event_id>` resolver in the attachment system.** Workable,
  and an Event ID selector may be valuable for attachments, but this feature
  does not need it: the click handler reads the source of truth directly.
  Deferred to a future RFD if an attachment-side need appears.
- **A built-in default such as `file://` pointing at `events.json`.** Opens the
  whole event log, base64 fields included, in whatever application the OS
  chooses.
  Worse than no link.
- **Linking the preparing row.** Needs an Event ID before the request is
  committed, which means an ID-reservation API on the stream.
  The row is on screen for seconds and then erased; the permanent header is the
  one that stays in scrollback.
- **Change what `jp conversation path --events` prints.** The command would
  print the copy JP loads without a flag.
  Rejected: `jp conversation edit` resolves its files the same way and has to
  open the workspace copy, and existing scripts would get a different path for
  every projected conversation.
- **Let the handler pick the copy.** The handler would compare modification
  times itself.
  `jp conversation path` cannot print the user-local copy of a projected
  conversation, so the handler would build that path from the layout of the user
  data directory and repeat JP's rule.

## Non-Goals

- Writing, storing, or garbage-collecting files.
- Changing `results_file_link`, inline result truncation, tool chrome layout, or
  request/response pairing.
- Registering URI scheme handlers or shipping click-handler scripts.
- A command that prints a stream entry by Event ID.
  It would shrink every handler, and it is worth its own proposal.
- Changing which copy `jp conversation edit` opens.
  Editing an older projection is a bug of its own ([T-1nf8vzp]).

## Risks and Open Questions

- **Capped responses.** [RFD 115] (Accepted) caps tool responses before they are
  persisted.
  Once it lands, a handler sees the response the assistant received, not the
  tool's full output; the full output is what `results_file_link` points at.
- **Pairing convenience.** Response pairing is handler-owned: the payload tool
  call `id`, scoped to the turn, is the matching rule.
  Request-response event linking ([RFD 098], in Discussion) narrows the
  candidates to one answer group but keeps the same rule.
  Helpful, not a prerequisite.
- **Uncheckable links.** JP cannot tell whether a handler exists, so a wrong
  template fails silently at click time.
  Placeholder validation catches a misspelled placeholder and nothing else.
- **A conversation that was never imported.** A conversation that exists only in
  one checkout's workspace projection resolves only from that checkout.
  The handler's loop over live checkouts finds it.
- **Checkouts can disagree.** `--loaded` prints the copy JP loads from the
  checkout it is asked in.
  A checkout whose projection was rewritten outside JP, by a `git pull` for
  example, loads that projection, while another checkout loads the user-local
  copy.
  The handler reads what the most recently used checkout would load.

## Implementation Plan

1. **Config field.** Add `header_link` to `DisplayStyleConfig` in `jp_config`,
   with the assignment, delta, fill, and partial plumbing its sibling fields
   have, and placeholder validation in the `ToolsConfig` validator.
   The field's type holds a link template or `false`, and its schema is a string
   or a boolean, as `results_file_link` already reads a boolean beside its style
   names.
   Mergeable alone; nothing changes while the key is unset.
2. **Live path.** Carry the request's Event ID from the commit to the header.
   The turn coordinator learns the ID when it pushes the `ToolCallRequest` and
   reports it with the committed request.
   The restart path, which resubmits requests read back from the stream, reads
   the ID from the same entries.
   The tool coordinator keeps the ID with each call and hands the substituted
   link to `ToolRenderer`, whose two header writers (`render_tool_call` and
   `render_custom_result`) wrap the tool name with `jp_term::osc::hyperlink`.
   Depends on phase 1.
3. **Replay parity.** `TurnRenderer` already iterates entries with their Event
   IDs.
   It resolves the link templates once from the invocation's configuration and
   keeps them outside the per-turn rebuild and the `--style` overlay (see [What
   is linked, and when](#what-is-linked-and-when)).
   Depends on phase 2.
4. **`jp conversation path --loaded`.** Add the flag.
   `--events` and `--base-config` resolve through the rule the loader applies to
   the stream (`Storage::stream_dir`, private to `jp_storage` today), and
   `--metadata` through the rule it applies to `metadata.json`.
   Tested with a projected conversation whose workspace copy is the older one,
   and one whose workspace copy is the newer one.
   Independent of phases 1 to 3; mergeable alone.
5. **Docs.** Document the key, the flag, and a worked click handler (a WezTerm
   `open-uri` hook and an OS scheme registration), including the loop over `jp w
   show`.
   Update the query stream pipeline wiki page (W-00dxyb6) where it describes
   tool rendering.
   Depends on phases 3 and 4.

## References

- [RFD 097]: stable stream-entry Event IDs, the identity this link carries.
- [RFD 087]: out-of-directory `--workspace` targeting and checkout enumeration
  (`jp w show`), which click handlers use.
- [RFD 031]: durable conversation storage, and the rule for which copy of a
  conversation JP loads.
- [RFD 047]: `jp conversation path`, which gains the `--loaded` flag.
- [RFD 096]: terminal output sanitization; `hyperlink` strips control characters
  with its `strip_controls`.
- [RFD 117]: tool call fan-out, the source of several headers per request.
- [RFD 115]: bounded tool output, which caps what `events.json` stores.
- [RFD 098]: request-response event linking, which narrows handler-side pairing
  once available.
- `crates/jp_cli/src/render/tool.rs`: the tool call header writers.
- `crates/jp_cli/src/render/turn.rs`: replay, its per-turn configuration, and
  the style overlay.
- `crates/jp_cli/src/cmd/query/turn_loop.rs`: where the committed
  `ToolCallRequest` reaches the tool coordinator.
- `crates/jp_cli/src/cmd/conversation/ls.rs`: the existing `jp://` links.
- `crates/jp_cli/src/cmd/conversation/path.rs`: `jp conversation path`.
- `crates/jp_storage/src/load.rs`: the rule that picks the copy JP loads.
- `crates/jp_term/src/osc.rs`: OSC 8 hyperlink emission.
- `crates/jp_config/src/conversation/tool/style.rs`: the per-tool style block
  this extends.

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
