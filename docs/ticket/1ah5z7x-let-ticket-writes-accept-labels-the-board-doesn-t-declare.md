# Let ticket writes accept labels the board doesn't declare

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-29
- **Label**: domain=plugins
- **Label**: relevant-rfd=116
- **Label**: type=enhancement

Asked to file a ticket tagged `foo=bar`, the assistant is refused when the board
doesn't declare `foo=bar` yet.
It can add the entry to `.jp/config.toml`, but `jp` reads configuration once per
`jp query` run, so the plugin keeps refusing the label until the next run.

## Proposal

A board option deciding whether writes accept undeclared labels, while the check
stays strict:

```toml
[plugins.command.ticket.options]
undeclared_labels = "accept"   # default "refuse", as today
```

- **With `accept`, writes take an undeclared label as written**: a new key, or a
  new value under a declared key.
  That covers `add`, `label`, `edit`, and the `ticket_*` tools.
- **Still refused:** malformed tokens; retired values, which are an explicit
  "no"; and the declared keys' `min`, `max`, and `unique`.
  An undeclared `type=epic` still counts toward `type`'s one-per-ticket bound.
- **The write says so:** "Added foo=bar, which the board doesn't declare; `jp
  ticket check` reports it until it is added to
  `plugins.command.ticket.options.labels`."
  The assistant sees this in the tool result.
- **`jp ticket check` and the docs build ignore the option** and report
  undeclared labels, so CI catches what's left behind.
- **The tool definitions** say undeclared labels are accepted.
  JP's carry no allowed-labels list, so nothing there blocks it.
- Set `accept` in JP's own configuration.

## Out of scope

Writing the new label into the configuration.
No host request lets a plugin write configuration, and `jp ticket` would have to
choose which layered file to edit.
RFD 078 records changes in a conversation's config, not the workspace's files,
so CI wouldn't see them.
In JP the assistant can add the entry with its file tools; a general "write to
workspace config" capability is a `jp` feature with its own RFD.

## Alternative considered

A per-call flag (`--allow-undeclared`, `allow_undeclared: true`).
More explicit, but the assistant still hits the refusal first and retries, which
is the friction this removes.
