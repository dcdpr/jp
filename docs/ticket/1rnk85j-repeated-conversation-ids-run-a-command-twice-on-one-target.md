# Repeated conversation IDs run a command twice on one target

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=bug

A conversation ID written twice, on the command line or on stdin, gives the
command two handles to the same conversation, and the command acts on it once
per handle:

```sh
jp c compact "$id" "$id" --reset=1   # removes compaction 1, then what was compaction 2
jp c compact "$id" "$id" --summary  # two paid summary requests, two stored compactions
printf '%s\n' "$id" "$id" | jp c print -   # prints the conversation twice
```

Raised in the review of #1391.

## Where the repeats come from

`resolve_request` in `jp_cli::cmd::target` resolves targets to IDs and acquires
one handle per ID.
Literal IDs keep every repeat (`resolve_targets`, the all-IDs branch), and
`read_stdin_ids` keeps them on purpose: its doc comment says order and
duplicates are preserved, so stdin matches the command line.
Keyword targets don't repeat: `+session` reads a history `activate` already
dedups, and `+pinned` / `+archived` come from maps.

Only `fork` wants a repeat: RFD 086 documents `jp c fork "$id" "$id"` as
creating two independent forks.
The two forks share a wall-clock ID until RFD 106 lands, the same collision
`fork A B` has.
Every other multi-target command is wrong on the second copy: `rm` and `archive`
would fail, the read-only commands print twice, and `compact` re-applies.

## Proposal

Drop repeated IDs in `resolve_request`, before handles are acquired, keeping
first-seen order.
`fork` opts out through its `ConversationLoadRequest` and keeps one handle per
occurrence.
Do it silently, with a `debug!` line per dropped ID: an error would break piping
IDs from overlapping files (`cat a.txt b.txt | jp c rm -`), and the intent of a
repeat is unambiguous.
Command-line and stdin targets keep behaving identically.

- [x] Dedup in `resolve_request`, in first-seen order
- [x] Update the `read_stdin_ids` doc comment, which promises duplicates are
      preserved
- [x] Test that targets `[a, a, b]` resolve to `[a, b]`
- [x] Regression test: a repeated ID with `--reset=1` removes one compaction
      event
- [x] Check `jp c unarchive`, which resolves its own targets instead of
      receiving handles
