# T0004: Command outcome messages emit prose in `--format=json`

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-15

Most commands report what they did with `Printer::println`, which wraps its
argument in an NDJSON envelope when the output format is JSON
(`crates/jp_printer/src/printer.rs:228-236`).

So a script asking for machine-readable output gets English to parse:

```console
$ jp c use jp-c17866928997 --format=json
{"message":"Switched active conversation from jp-c17867259582 to jp-c17866928997: Tool Chaining"}

$ jp c fork jp-c17866928997 --format=json
{"message":"Conversation forked."}
```

The second is worse than the first: the new conversation's ID is not in the
output at all, in any format, so a script cannot chain off a fork.

## The mechanism already exists

`jp c label` was converted in PR \#982 and emits structured objects:

```console
$ jp c label add team=platform --format=json
{"action":"added","conversation":{"id":"jp-c17866928997","title":"Tool Chaining"},"labels":[{"key":"team","value":"platform"}]}
```

It uses `output::print_outcome(printer, text, json)`, which prints prose for the
text formats and the object for the JSON ones.
The two forms are supplied separately because they carry different things — the
same split `DetailItem` already makes for list entries.

`print_outcome` sits alongside `print_table`, `print_details`, and `print_json`
in `crates/jp_cli/src/output.rs`.

## Sites

Outcome messages, one per line, all currently prose-in-JSON:

- `cmd/init.rs:87` — "Initialized workspace at {loc}"
- `cmd/attachment/ls.rs:14` — "No attachments in current context."
- `cmd/config/fmt.rs:67` — formatting result
- `cmd/conversation/archive.rs:79,101` — no-match, and "Conversation {id}
  archived."
- `cmd/conversation/compact.rs:716,755,769,796,872,883` — "Nothing to compact."
  and the applied-compaction lines
- `cmd/conversation/edit.rs:306` — "Conversation(s) updated."
- `cmd/conversation/fork.rs:190` — "Conversation forked."
- `cmd/conversation/path.rs:45` — the resolved path
- `cmd/conversation/rm.rs:51,63` — no-match, and "Conversation(s) removed."
- `cmd/conversation/unarchive.rs:36,42,57` — skip, unarchived, and no-archived
- `cmd/conversation/use_.rs:140` — "Switched active conversation from X to Y:
  Title"
- `cmd/query.rs:464` — "Query is empty, ignoring."

Twelve commands.

## Two things to decide while converting

**Empty results should be empty collections, not messages.** `jp a ls` prints a
details table when attachments exist and `{"message":"No attachments in current
context."}` when they don't, so the JSON shape changes with the result.
An empty listing should be `[]` or an empty object.
The same applies to the no-match lines in `archive`, `rm`, and `unarchive`.

**`jp c path` is a value, not a status.** It prints a path for a script to
consume.
`{"path":"/..."}` is the obvious shape, but it is a different kind of output
from the rest of this list and worth treating as such.

## Out of scope

- `render/chat.rs` and `render/structured.rs` — streaming assistant content,
  not command outcomes.
- `cmd/attachment/print.rs`, `cmd/config/show.rs` — print content the user
  asked for verbatim.
- `cmd/conversation/label.rs:190` — the collapsed multi-target line is
  text-only by design; JSON never collapses and emits one object per
  conversation.

## Why it is filed rather than fixed in place

Raised while converting `jp c label` in PR \#982.
Converting twelve commands touches their tests and forces the empty-collection
decision above, which is a change of its own rather than a rider on a labels PR.

## Severity

Contained and visible: text output is unaffected, and a JSON consumer sees
well-formed JSON — just with a sentence where fields should be.
No data is lost except in `jp c fork`, which never reports the new
conversation's ID in any format.

## Comments

-----

- **From**: jp
- **Date**: 2026-08-15T05:39:38Z

While converting these, look for a way to make the JSON form a type-level
requirement rather than a convention.

`print_outcome(printer, text, json)` is the right mechanism but the wrong
enforcement: nothing stops the next command from reaching for `printer.println`
and re-introducing a `{"message": "..."}` envelope.
Every site on the list above is a place someone did exactly that, so convention
alone has already failed once per command.

The obvious lever is the return type.
Commands return `Output = Result<(), Error>` (`crates/jp_cli/src/cmd.rs:182`),
which says nothing about what they reported.
If the success arm carried the outcome instead — something along the lines of
`Result<Outcome, Error>`, where `Outcome` owns both a `Display` form and a
`Value` form — then a command that reports nothing structured would not
compile, and rendering would move to one place in `run_inner` rather than being
repeated per command.

That shape has other consequences worth weighing before committing to it:

- It removes the printer from most command bodies, which makes outcomes directly
  assertable in tests without a `SharedBuffer`.
- It forces a decision about commands that legitimately emit *nothing* (`jp c
  print` streams content; `jp q` renders a turn), so `Outcome` probably needs an
  explicit "nothing to report" variant rather than allowing `()`.
- It does not cover streaming or progress output, which stays on the printer.
  The boundary between "the result" and "output produced along the way" needs
  stating, or the type will be bypassed for the same reason `println` is reached
  for today.
- Multi-target commands emit one record per conversation, so the success value
  is plural for some commands and singular for others.

Cheaper intermediate options, if the full change is too large: make the
outcome-reporting helpers the only `pub` printing surface in `output.rs` and
restrict `Printer::println` to `pub(crate)` within the render modules, or add a
lint/test that fails when a command module references `printer.println`
directly.

This is a design question rather than a mechanical one, so it may deserve its
own RFD if the return-type change is the direction.
Worth deciding *before* converting twelve commands by hand, since the conversion
is the natural moment to change the signature and doing it twice would be
wasteful.
