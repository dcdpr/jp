# RFD 086: Line-oriented stdin input for CLI arguments

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-17
- **Extends**: [RFD 048]

## Summary

This RFD establishes a project-wide CLI convention with two clauses:

1. For a **multi-value argument that accepts stdin input**, the value `-` means
   "read this argument's values from standard input."
2. That stdin input is line-oriented: one value per line.

The rule is a convention, not a command feature: any multi-value argument that
chooses to read stdin does so via `-`, line-oriented — there is no second
spelling, and the rule does not require every argument to read stdin.
Its first and only opt-in so far is conversation targeting — `jp c archive -`,
`jp c rm -`, and the other multi-ID commands — which lets the ID-list output of
one command pipe straight into the next.

Single-value arguments are out of scope: `-` is rejected on them for now (see
[Non-Goals](#non-goals)).

## Motivation

JP's management commands (`archive`, `rm`, `print`, `path`) take one or more
conversation IDs.
Today the only ways to supply them are literal IDs on the command line, keywords
(`+pinned`, `+session`), or the interactive picker.
There is no way to feed a *computed* set of IDs — "every conversation whose
title matches `\Apr-triage:\d{3}\z`" — without hand-copying IDs out of one
command and into another.

The motivating workflow is archiving a batch of conversations selected by a
pattern:

```sh
jp c grep -l --regex '\Apr-triage:\d{3}\z' | jp c archive -
```

[RFD 050] defines the *production* half of this story for `conversation new` and
`conversation fork`: they print one conversation ID per line (JSON array under
`-F json`).
`jp c grep --list` emits the same line-oriented ID format.
This RFD defines the *consumption* half — the matching input convention — so
the line-oriented ID output of one JP command is valid input to another with no
`jq`/`xargs` glue.

Doing nothing leaves two costs.
Users assemble per-command shell glue (`... | jq -r '.[].id' | xargs ...`) that
leaks JP's opaque ID format into scripts, and every command that might one day
read stdin reinvents the handling ad hoc, with no shared rule for what `-`
means.

## Design

### The convention

For a multi-value argument that accepts stdin input, the value `-` means "read
this argument's values from stdin," one value per line.
It applies whether the argument is positional (`jp c archive -`) or a
multi-value flag.
Phase 1 opts in only conversation targeting; other multi-value arguments
(`--scope`, `--mount`, …) keep their current behavior until they explicitly opt
in.
It is the rule users already know from `cat -`, `git`, and most Unix tools
(Principle of Least Astonishment).

The format `-` consumes is the **line-oriented ID list**: one opaque
conversation ID per nonblank line.
This is the same format `conversation new`, `conversation fork`, and `grep
--list` emit in text mode — a deliberate, stable machine format, distinct from
the human-facing rendered output that JP changes freely.
`-` does *not* consume JSON: `-F json` emits an array, which a tool transforms
(`jq -r '.[].id'`) before piping into `-`.

### What the user sees

```sh
# positional, multi-ID
jp c grep -l --regex '\Apr-triage:\d{3}\z' | jp c archive -

# explicit producer, same shape
jp c ls -F json | jq -r '.[].id' | jp c rm -
```

The IDs arrive resolved before the command runs, exactly as if typed on the
command line.
Commands stay ignorant of stdin — `archive` and `rm` never read it themselves.

### Where the read happens

Reading stdin is I/O and belongs in the imperative shell, not in argument
parsing.
So `-` parses to a **sentinel**, and the read happens once, later, when the
shell resolves the argument:

- Parsing records *"stdin requested"* — no I/O, parsing stays pure.
- The shell reads stdin a single time, splits on newlines, normalizes, and
  expands the sentinel into concrete values.

For conversation targeting this slots into the existing seam with no new
machinery.
`ConversationTarget::parse` maps `-` to a new `Stdin` variant, and
`ConversationTarget::resolve` (in `cmd/target.rs`) reads stdin and resolves it,
the same way `AllSession` resolves one token into many IDs.

### Affected commands

`resolve` — not `resolve_request` — is the seam.
Every path that turns a target into IDs funnels through it, including the
commands that resolve targets internally, so `-` reaches every
conversation-target command, not only the four in the motivation:

| Command(s)                    | Resolution path        | Notes                             |
| ----------------------------- | ---------------------- | --------------------------------- |
| `archive`, `rm`               | `resolve_request`      | destructive; confirmation applies |
| `print`, `path`, `ls`, `show` | `resolve_request`      | read-only                         |
| `edit`, `fork`, `compact`     | `resolve_request`      | mutate state                      |
| `grep`                        | `resolve_request`      | producer and consumer both        |
| `unarchive`                   | internal `resolve_ids` | bypasses `resolve_request`, but   |
|                               |                        | still calls `resolve`             |

This breadth is intended: `-` is a generic targeting source, so a reader has no
reason to expect `print -` to work but `show -` not to.

The invariant the implementation must hold: every command that turns targets
into IDs forwards non-`Id` targets to `ConversationTarget::resolve` rather than
pattern-matching `Id` and dropping the rest.
A command that silently ignores a `Stdin` target would accept `-` and do
nothing.
`unarchive` already satisfies this; new commands must too.

### Normalization

Two layers, kept separate so the global convention does not constrain future
argument types:

**Global convention** — true for any argument that opts into stdin input:

- `-` reads values from stdin.
- Input is line-oriented: one value per line, unless the argument defines
  another format.

**Conversation-ID behavior** — specific to the conversation-target consumer:

- Trim each line; drop blank lines.
  Dropping blanks is required, not cosmetic: a well-formed pipe ends in a
  trailing newline (`printf 'id1\nid2\n'`), so without it the stream would yield
  a spurious empty final value.
- Parse each nonblank line as a `ConversationId`, with the same error an
  unparseable ID produces on the command line.
- Preserve order *and* duplicates.
  The resolved list is exactly the lines, so `printf 'jp-c1\njp-c1\n' | jp c
  fork -` forks twice — identical to `jp c fork jp-c1 jp-c1`.
  Commands that want set semantics deduplicate themselves, as they already must
  for repeated command-line IDs.

Trimming and blank-line dropping are conversation-ID conveniences, not global
guarantees: an argument with whitespace- or order-significant values would
define its own stdin parsing instead.

### Edge cases

- **Empty input.** Because blank lines are dropped, an empty or whitespace-only
  stdin resolves to *zero* values.
  This is an explicit error (*"no conversation IDs provided on stdin"*), not a
  silent no-op — a mistyped upstream filter fails loudly instead of quietly
  doing nothing.
- **Terminal stdin.** If `-` is requested but stdin is a terminal (nothing
  piped), the command errors rather than blocking on keyboard EOF.
  This mirrors the existing picker guards in target resolution
  (`resolve_picker`, `resolve_multi_picker`), which already refuse picker
  fallback when stdin is not a terminal.
- **No mixing.** `-` must be the sole target.
  Combining it with literal IDs (`jp c archive id1 -`) or with keywords and
  pickers (`+pinned`, `?`) is rejected.
  A request either takes its IDs from the command line or from stdin, never both
  — which keeps resolution unambiguous and sidesteps ordering questions.

### Single-consumer invariant

Stdin can be drained once, so at most one consumer may read it per invocation.
`jp query` already reads piped stdin *implicitly* and prepends it to the prompt
(`cmd/query.rs`); that is a pre-existing, command-specific consumer.
`query` takes no multi-ID argument, so there is no live collision — but the
invariant is what guarantees that stays true as commands evolve.
A command that grows both a `-` argument and content-from-stdin must fail that
combination rather than race over a single stream.

### Relationship to RFD 048

[RFD 048] defines JP's channel model: stdin is the *data* channel, never the
*prompt* channel.
Prompts and interactive input always come from `/dev/tty` (or the detached
policy in [RFD 049]), so `echo "y" | jp query "..."` is query content, not a
prompt answer.

This RFD does not touch that rule.
It refines what the data channel *carries*: stdin supplies either implicit query
content (`jp query`, unchanged) or the explicit `-` argument values defined
here, and at most one of them per invocation.
The prompt-channel guarantee from RFD 048 is preserved intact — which is why
this RFD extends RFD 048 rather than contradicting it.

### Convention now, generic mechanism later

This RFD fixes the *convention* for the whole CLI.
The *mechanism* starts minimal: the only consumer is conversation targeting, so
the sentinel lives in `ConversationTarget`.
A shared, command-agnostic stdin-argument facility is worth extracting only when
a second, non-conversation consumer appears (YAGNI, and the midlayer mistake).
Until then, the documented rule is what keeps future arguments consistent.

## Drawbacks

- **Two stdin models coexist.** `jp query` consumes stdin implicitly (no `-`);
  everything else is explicit (`-` opts in).
  That is a deliberate inconsistency — `query`'s behavior predates this
  convention and is relied on for interactive use, so it is grandfathered rather
  than changed.
  New stdin consumers follow the explicit rule.
- **A convention is only as good as adherence.** Nothing in the type system
  forces a future multi-value argument to honor `-`.
  The rule lives in docs and review until a shared mechanism exists, so there is
  drift risk.
- **`-` becomes reserved for stdin-capable arguments.** A multi-value argument
  that opts into stdin input cannot also use bare `-` as a literal value.
  Single-value flags with their own established `-` meaning are unaffected —
  `--log-file=-` (write tracing to stderr, [RFD 048]) keeps working.
  A future stdin-capable argument that genuinely needs literal `-` would have to
  define an escape or alternate spelling.

## Alternatives

### Per-command stdin handling

Each command reads stdin itself.
Rejected: it duplicates parsing across `archive`/`rm`/`print`/`path` and punches
an I/O hole through the clean "commands receive resolved handles" boundary.
One shortcut compounds into N.

### A conversation-only `Stdin` target, without stating the general rule

The mechanism would be identical, but framing it as a conversation feature
leaves the next argument that wants stdin input to reinvent what `-` means.
Stating the rule once is the difference; the implementation is the same first
step either way.

### Implicit stdin for all commands (the `query` model, generalized)

Every command auto-reads piped stdin.
Rejected: it is ambiguous (a command can't tell "piped IDs" from "piped
content"), it can't coexist with the interactive picker, and it makes stdin
consumption invisible at the call site.
Explicit `-` is the safer default.

### `xargs` / `jq` at the shell level

Works today without any JP change, but it is per-command glue that leaks the
opaque ID format into user scripts and gives up JP-side validation and
confirmation (e.g. titles in the `archive` prompt).
The convention makes the common case glue-free.

## Non-Goals

- **Single-value arguments.** `-` is accepted only for multi-value arguments;
  single-value targets reject it for now.
  The motivating workflows use multi-value commands (`archive`, `rm`), where a
  one-line stdin is simply a one-element list — so single-value stdin buys
  little beyond `jp query --id -`, which is also the one command that already
  owns stdin for prompt content.
  Designing that arbitration (inline query vs. editor vs. stdin) belongs in a
  dedicated query-input RFD, not here.
- **Changing `jp query`'s prompt-from-stdin behavior.** It keeps reading piped
  stdin as prompt content.
  This RFD governs *argument values*, not that pre-existing consumer.
- **Building a generic cross-command stdin-argument framework now.** The
  convention is stated; the shared mechanism is extracted when a second consumer
  needs it.
- **The producer side.** `jp c grep`'s regex matching and `--list` output, which
  feed the pipeline above, emit the line-oriented ID format but are implemented
  as separate work, not specified by this convention.

## Implementation Plan

### Phase 1: `-` for conversation targeting

- Add `ConversationTarget::Stdin`; `ConversationTarget::parse` maps `-` to it.
- In `ConversationTarget::resolve` (`cmd/target.rs`), read stdin once for a
  `Stdin` target: split on newlines, trim, drop blanks, parse each as a
  `ConversationId` (order and duplicates preserved), with the terminal-stdin
  guard and the empty-input error.
- Extend `validate_multi` (`cmd/conversation_id.rs`) so `-` is rejected when
  combined with any other target, and rejected on single-value targets.
- Available to every conversation-target command at once, because they all
  funnel non-`Id` targets through `resolve` — including `unarchive`, which
  resolves internally.
- Audit every prompt reachable after stdin resolution — the `archive` and `rm`
  confirmations and the target pickers.
  Input is already safe: `inquire` reads the terminal (`/dev/tty`), not stdin,
  so draining stdin for IDs does not starve the prompt.
  Output needs care: prompts render via `Printer::prompt_writer`, which renders
  to `/dev/tty` when one is open but **falls back to stdout** when none is —
  and for these stdin-targeted commands stdout is the data channel a downstream
  pipe consumes.
  So when no prompt channel is available, the command must follow the [RFD 049]
  detached policy (e.g. require `--yes`) rather than writing the prompt to
  stdout and proceeding.
  Changing `prompt_writer`'s fallback globally is out of scope — other callers
  (the init wizard, lock prompts) may want it; the guard belongs on the
  destructive commands when they adopt RFD 049's non-interactive handling.
- Independently mergeable.

### Phase 2 (deferred): shared stdin-argument facility

Extract a command-agnostic helper only when a non-conversation multi-value
argument needs `-`.
Not built now.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — defines the stdin data
  channel vs. the `/dev/tty` prompt channel that this RFD extends.
- [RFD 049: Non-Interactive Mode][RFD 049] — the detached prompt policy
  referenced by the channel model.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050] —
  defines the one-ID-per-line / JSON-array ID *output* for `new`/`fork` that
  this RFD mirrors as *input*.

[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
