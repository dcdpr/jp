# RFD 086: Line-Oriented Stdin Input for CLI Arguments

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-17
- **Extends**: [RFD 048]
- **Summary**: Establishes the CLI convention that `-` on a multi-value argument
  reads newline-delimited values from stdin.

## Summary

This RFD establishes a project-wide CLI convention: when a multi-value argument
accepts standard input, passing `-` reads one value per line from stdin.
Conversation targeting (`jp c archive -`, `jp c rm -`, and other multi-ID
commands) is the first adopter, so ID lists from commands like `jp c grep
--output ids` pipe directly into management subcommands.
Single-value arguments are out of scope and reject `-`.

## Motivation

JP's management commands (`archive`, `rm`, `print`, `path`) take one or more
conversation IDs via command-line arguments, keywords (`+pinned`, `+session`),
or the interactive picker.
There is no way to pass a computed set of IDs, such as every conversation whose
title matches `\Apr-triage:\d{3}\z`, without copying IDs by hand from one
command into another.

The motivating workflow is archiving a batch of conversations selected by a
pattern:

```sh
jp c grep --output ids --regex '\Apr-triage:\d{3}\z' | jp c archive -
```

[RFD 050] defines the output convention for `conversation new` and `conversation
fork`: they print one conversation ID per line (or a JSON array under `-F
json`), and `jp c grep --output ids` emits the same line-oriented ID format.
This RFD defines the matching input convention so the line-oriented ID output of
one JP command pipes directly into another without `jq` or `xargs` glue.

Without a shared convention, users must write per-command shell glue (`... | jq
-r '.[].id' | xargs ...`) that leaks JP's opaque ID format into scripts, and
each command that reads stdin has to invent its own `-` handling.

## Design

### Convention

For any multi-value argument that accepts stdin input, passing `-` reads the
argument's values from stdin, one value per line.
This applies to both positional arguments (`jp c archive -`) and multi-value
flags, matching the `-` convention in `cat -`, `git`, and other Unix tools
(Principle of Least Astonishment).
Phase 1 opts in only conversation targeting; other multi-value arguments
(`--scope`, `--mount`, and others) keep their current behavior until they
explicitly opt in.

For conversation targeting, `-` consumes a **line-oriented ID list** with one
opaque conversation ID per nonblank line.
`conversation new`, `conversation fork`, and `grep --output ids` emit this same
format in text mode as a stable machine interface, distinct from human-facing
rendered output that JP changes freely.
`-` does not parse JSON: `-F json` emits a JSON array, which callers transform
with a tool such as `jq -r '.[].id'` before piping into `-`.

### CLI behavior

```sh
# positional, multi-ID
jp c grep --output ids --regex '\Apr-triage:\d{3}\z' | jp c archive -

# explicit producer, same shape
jp c ls -F json | jq -r '.[].id' | jp c rm -
```

IDs read from stdin are resolved before the command runs, just as if they were
passed on the command line.
Individual commands such as `archive` and `rm` never read stdin themselves.

### Where stdin is read

Reading stdin is I/O and belongs in the imperative shell rather than in argument
parsing.
Argument parsing stays pure by mapping `-` to a **sentinel** that records the
request to read stdin.
When the shell later resolves the argument, it reads stdin once, splits on
newlines, normalizes each line, and expands the sentinel into concrete values.

For conversation targeting, this fits the existing resolution seam without new
machinery.
`ConversationTarget::parse` maps `-` to a `Stdin` variant, and
`ConversationTarget::resolve` (in `cmd/target.rs`) reads stdin and expands it
into conversation IDs, just as `AllSession` expands a single token into multiple
IDs.

### Affected commands

`ConversationTarget::resolve` is the resolution seam rather than
`resolve_request`.
Every path that turns a target into IDs calls `resolve`, including commands that
resolve targets internally, so `-` works across all conversation-target
commands:

| Command(s)                    | Resolution path        | Notes                             |
| ----------------------------- | ---------------------- | --------------------------------- |
| `archive`, `rm`               | `resolve_request`      | destructive; confirmation applies |
| `print`, `path`, `ls`, `show` | `resolve_request`      | read-only                         |
| `edit`, `fork`, `compact`     | `resolve_request`      | mutate state                      |
| `grep`                        | `resolve_request`      | producer and consumer both        |
| `unarchive`                   | internal `resolve_ids` | bypasses `resolve_request`, but   |
|                               |                        | still calls `resolve`             |

This breadth is intentional: `-` is a general targeting source, so `show -`
works the same way as `print -`.

To preserve this behavior, every command that resolves targets into IDs must
forward non-`Id` targets to `ConversationTarget::resolve` rather than
pattern-matching `Id` and dropping other variants.
Otherwise, a command would accept `-` and silently do nothing.
`unarchive` already satisfies this invariant, and new commands must as well.

### Normalization

Normalization is split into two layers so the global convention does not
constrain future argument types:

**Global convention** (applies to any argument that opts into stdin input):

- `-` reads values from stdin.
- Input is line-oriented: one value per line, unless the argument defines
  another format.

**Conversation-ID behavior** (specific to conversation targeting):

- Trim whitespace from each line and drop blank lines.
  Dropping blank lines is required because a standard Unix pipe ends with a
  trailing newline (`printf 'id1\nid2\n'`), which would otherwise produce a
  spurious empty final value.
- Parse each nonblank line as a `ConversationId`, reporting the same error that
  an unparseable ID produces on the command line.
- Preserve order and duplicates.
  `printf 'jp-c1\njp-c1\n' | jp c fork -` forks `jp-c1` twice, matching `jp c
  fork jp-c1 jp-c1`.
  Commands that require set semantics deduplicate the resolved IDs themselves,
  just as they do for repeated command-line IDs.

Trimming whitespace and dropping blank lines apply only to conversation IDs; an
argument with whitespace- or order-significant values can define its own stdin
parsing.

### Edge cases

- **Empty input.** Because blank lines are dropped, an empty or whitespace-only
  stdin resolves to zero values.
  Target resolution returns an explicit error (*"no conversation IDs provided on
  stdin"*) rather than succeeding as a silent no-op, so a mistyped upstream
  filter fails loudly.
- **Terminal stdin.** If `-` is passed when stdin is a terminal (nothing piped),
  the command returns an error instead of blocking on keyboard EOF.
  This matches the picker guards in target resolution (`resolve_picker`,
  `resolve_multi_picker`), which reject picker fallback when stdin is not a
  terminal.
- **No mixing.** `-` must be the sole target.
  Combining `-` with literal IDs (`jp c archive id1 -`), keywords (`+pinned`),
  or pickers (`?`) is rejected.
  Taking IDs from either the command line or stdin, never both, keeps resolution
  unambiguous and avoids ordering questions.

### Single-consumer invariant

Stdin can be drained only once, so at most one consumer may read it per
invocation.
`jp query` already reads piped stdin implicitly and prepends it to the prompt
(`cmd/query.rs`).
Because `query` takes no multi-ID argument, the two consumers do not collide,
and this invariant ensures they cannot as commands evolve: any command that
supports both a `-` argument and content from stdin must reject combining them
rather than competing for a single stream.

### Relationship to RFD 048

[RFD 048] defines JP's channel model: stdin is the *data* channel, never the
*prompt* channel.
Prompts and interactive input always come from `/dev/tty` (or follow the
detached policy in [RFD 049]), so `echo "y" | jp query "..."` is treated as
query content rather than a prompt answer.

This RFD extends [RFD 048] by refining what the data channel carries while
preserving the prompt-channel guarantee: stdin supplies either implicit query
content (`jp query`, unchanged) or explicit `-` argument values, and at most one
of the two per invocation.

### Convention now, generic mechanism later

This RFD sets the convention for the entire CLI while keeping the initial
mechanism minimal.
Because conversation targeting is the only consumer in Phase 1, the sentinel
lives in `ConversationTarget`.
Extracting a shared, command-agnostic stdin-argument facility is deferred until
a second, non-conversation consumer needs it (YAGNI, avoiding a premature
midlayer).
Until then, the documented convention keeps future arguments consistent.

## Drawbacks

- **Two stdin models coexist.** `jp query` consumes stdin implicitly without
  `-`, while all other commands require an explicit `-`.
  Because `query`'s behavior predates this convention and is relied on for
  interactive use, it is preserved; all new stdin consumers follow the explicit
  `-` rule.
- **No type-system enforcement yet.** Until a shared mechanism exists, nothing
  in the type system forces a new multi-value argument to support `-`
  consistently, so adherence depends on documentation and review.
- **`-` is reserved on stdin-capable arguments.** A multi-value argument that
  opts into stdin input cannot also treat bare `-` as a literal value, and would
  need an escape or alternate spelling if literal `-` were required.
  Single-value flags with their own established `-` meaning, such as
  `--log-file=-` (write tracing to stderr, [RFD 048]), are unaffected.

## Alternatives

### Per-command stdin handling

Have each command read stdin itself.
Rejected because it duplicates parsing across `archive`, `rm`, `print`, and
`path`, and breaks the boundary where commands receive already-resolved handles.

### Conversation-only `Stdin` target without a general convention

Implement `ConversationTarget::Stdin` as a conversation-specific feature without
defining a CLI-wide rule.
The Phase 1 implementation is identical either way, but stating the convention
now prevents future stdin-capable arguments from reinventing what `-` means.

### Implicit stdin for all commands

Automatically read piped stdin in every command, generalizing `jp query`'s
model.
Rejected because a command cannot distinguish piped IDs from piped content,
implicit reading cannot coexist with the interactive picker, and stdin
consumption becomes invisible at the call site.

### `xargs` and `jq` at the shell level

Rely on shell glue without changing JP.
This works without CLI changes, but it requires per-command boilerplate that
leaks JP's opaque ID format into user scripts and bypasses JP-side validation
and confirmation prompts (such as showing conversation titles in the `archive`
prompt).

## Non-Goals

- **Single-value arguments.** `-` is accepted only for multi-value arguments;
  single-value targets reject it.
  The motivating workflows use multi-value commands (`archive`, `rm`), where a
  one-line stdin stream is handled as a one-element list.
  Single-value stdin would mainly affect `jp query --id -`, where stdin is
  already used for prompt content; arbitrating between inline queries, the
  editor, and stdin belongs in a dedicated query-input RFD.
- **Changing `jp query`'s prompt-from-stdin behavior.** `jp query` continues to
  read piped stdin as prompt content.
  This RFD governs explicit `-` argument values, not that pre-existing consumer.
- **Building a generic cross-command stdin-argument framework now.** Phase 1
  establishes the convention and implements it for conversation targeting; a
  shared helper is deferred until a second consumer needs it.
- **Producer commands.** `jp c grep`'s regex matching and `--output ids` flag
  emit the line-oriented ID format consumed here, but are implemented as
  separate work.

## Implementation Plan

### Phase 1: `-` for conversation targeting

- Add `ConversationTarget::Stdin` and map `-` to it in
  `ConversationTarget::parse`.
- In `ConversationTarget::resolve` (`cmd/target.rs`), read stdin once for a
  `Stdin` target: reject terminal stdin, split on newlines, trim whitespace,
  drop blank lines, error on empty input, and parse each line as a
  `ConversationId` while preserving order and duplicates.
  Because every conversation-target command funnels non-`Id` targets through
  `resolve` (including `unarchive`, which resolves internally), this enables `-`
  across all of them at once.
- Extend `validate_multi` (`cmd/conversation_id.rs`) so `-` is rejected on
  single-value targets and when combined with any other target.
- Audit every prompt reachable after stdin resolution, including the `archive`
  and `rm` confirmations and the target pickers.
  Prompt input is already safe because `inquire` reads `/dev/tty` rather than
  stdin, so draining stdin for IDs does not starve the prompt.
  Prompt output requires a guard: `Printer::prompt_writer` writes to `/dev/tty`
  when available but **falls back to stdout** when no terminal is open, and for
  stdin-targeted commands stdout is the data channel a downstream pipe consumes.
  When no prompt channel is available, destructive commands must follow the [RFD
  049] detached policy (for example, requiring `--yes`) rather than writing the
  prompt to stdout and proceeding.
  Changing `prompt_writer`'s fallback globally is out of scope because other
  callers (such as the init wizard and lock prompts) rely on it.
- Phase 1 is independently mergeable.

### Phase 2 (deferred): shared stdin-argument facility

Extract a command-agnostic helper once a non-conversation multi-value argument
needs `-`.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: Defines the stdin data channel
  and `/dev/tty` prompt channel that this RFD extends.
- [RFD 049: Non-Interactive Mode][RFD 049]: Defines the detached prompt policy
  referenced by the channel model.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050]: Defines
  the line-oriented and JSON-array ID output for `new` and `fork` that this RFD
  mirrors as input.

[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
