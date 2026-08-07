# RFD D52: Conversation Labels

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-06

## Summary

Conversations carry labels: `key=value` pairs stored in conversation metadata.
Labels are declared under `conversation.labels.<name>`; the map key is the label
key.
A value is a static string or a command's stdout, resolved at conversation
creation.
Labels marked `apply_on = "manual"` are not applied automatically; they are
named aliases for `--label`.

## Motivation

A conversation carries a title, timestamps, and three fixed flags: pinned,
archived, and user-local.
Every one of these is an axis JP chose.
None of them lets a user group conversations along an axis of their own.

The concrete case is version control.
A user who works across several branches wants to find the conversations started
while on one of them.
JP is VCS-agnostic and cannot read a branch name itself, so the value has to
come from a command the user supplies.

[RFD 040] deferred this deliberately.
It added a `hidden` flag and recorded general-purpose classification as future
work, noting that `hidden` can migrate to a label once labels exist.

## Design

### Data Model

`Conversation` in `jp_conversation` gains one field:

```rust
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub labels: BTreeMap<String, String>,
```

It sits beside `title`, `pinned_at`, and `archived_at`, and persists to
`metadata.json` with them.
A missing field deserializes to an empty map, so existing conversations need no
migration.
Ordering is deterministic because `BTreeMap` sorts by key, which keeps
`metadata.json` diffs stable.

There is no separate representation for a label without a value.
A key whose value is the empty string is the bare-label idiom, and filters match
it on key presence alone.

### Config Shape

`conversation.labels` is a `MergeableMap<LabelConfig>`, so it merges like every
other keyed config map and accepts an explicit strategy.
The map key is the label key.
There is no `key` field.

A `LabelConfig` is either a bare string or an object:

```rust
#[serde(untagged)]
enum LabelConfig {
    Static(String),
    Object(LabelObject),
}

struct LabelObject {
    value: Option<String>,
    cmd: Option<CommandConfigOrString>,
    apply_on: ApplyOn,
}
```

`value` holds a static string.
`cmd` holds a command whose stdout becomes the value.
Setting both is a configuration error.
Setting neither yields the empty value.

| TOML                                                                        | Result                         |
| --------------------------------------------------------------------------- | ------------------------------ |
| `labels.foo = "bar"`                                                        | static `foo=bar`               |
| `labels.foo = ""`                                                           | static `foo=`                  |
| `labels.foo = {}`                                                           | static `foo=`                  |
| `labels.foo.cmd = "git branch --show-current"`                              | dynamic, string-shaped command |
| `labels.foo.cmd = { program = "git", args = ["branch", "--show-current"] }` | dynamic, structured command    |
| `labels.foo = { value = "x", apply_on = "manual" }`                         | static, alias only             |
| `labels.foo = { cmd = "...", apply_on = "turn" }`                           | dynamic, re-resolved per turn  |
| `labels.foo = { value = "x", cmd = "..." }`                                 | configuration error            |

`apply_on` takes three values and defaults to `new`:

- `new`: apply once, when the conversation is created.
- `turn`: re-resolve at the start of every turn.
- `manual`: never apply automatically.
  The declaration becomes an alias for `--label`.

Label keys cannot contain `=`, which the CLI uses as a separator, and cannot
begin with `:`, which marks an alias reference.
Any other Unicode is accepted.
Values are unconstrained.

### Resolution

Resolution lives in `jp_workspace`, beside conversation creation.
The data type stays in `jp_conversation`, which performs no I/O.

At creation, the workspace resolves every label whose `apply_on` is `new`:

1. Static declarations yield their value directly.
2. Command declarations run in parallel from the workspace root; trimmed stdout
   becomes the value.
3. `--label` flags apply last and overwrite config-resolved values.

A command that fails or writes nothing produces a warning and no label; creation
continues.
A command declaring `shell = true` is rejected at config load as a footgun
guard, not a trust boundary; `program = "sh"` reaches the same place.

Labels with `apply_on = "turn"` re-resolve at the start of each turn, before the
request reaches the provider.
Only the current value is stored; no history is kept.
A fork inherits its parent's labels verbatim and re-runs nothing.

### CLI Surface

```sh
jp query --new --label=team=ops --label=branch=main
jp query --new --label=:release
jp conversation edit --label=reviewed --no-label=wip
jp conversation ls --label=branch=main --label=team
jp conversation grep --label=team=ops "retry policy"
```

`--label` is repeatable.
Three forms are accepted: `foo` sets the empty value, `foo=bar` sets a value,
and `:name` resolves a declaration from `conversation.labels`.
A `manual` declaration carrying `cmd` runs that command when the flag resolves.

`--no-label=KEY` removes a label on `edit`.
Setting a label that already exists replaces its value.

On `ls` and `grep`, `--label` filters instead of setting.
`foo` matches any conversation carrying that key; `foo=bar` matches an exact
pair.
Repeated flags combine with AND.

`conversation show` renders the resolved labels.
`ls` does not, because its table is already wide.

## Alternatives

**Array-of-tables with a `name` field.** `[[conversation.labels]]` with an
explicit `name` diverges from `conversation.tools.<name>`, and array merging
needs matching rules that keyed maps get free.

**Nesting the command under `value`.** Making `value` an untagged
string-or-command union adds a second enum for nothing; `value` and `cmd` are
mutually exclusive either way.

**A `run` field on `CommandConfig`.** Confirmation policy on the command type
collides with consumer-level `run` and still needs a default, so it prevents no
mistake.

**Bare labels as a distinct type.** A `BTreeSet` beside the map doubles the
flag, filter, and serialized surface to carry what the empty string already
carries.

**Event-sourced labels.** `Label` and `Unlabel` events buy an unrequested audit
trail, turn the current set into a fold, and slow every listing.

**Static values from the environment.** Requiring users to export a branch
variable moves the problem into shell configuration.

## Non-Goals

- **Multi-key command output.** One command emitting several labels at once.
  Deferred; declare one entry per label.

- **`shell = true` in label commands.** Rejected for v1.
  The configuration loader refuses it.

- **Command timeouts and sandboxing.** Deferred to the trust-policy work.

- **Tool access to labels.** Deferred; exposing labels to tools needs per-label
  opt-in, because values carry sensitive data.

- **Labels in `conversation ls` output.** Rejected; the table is already too
  wide.

- **Filter negation beyond `--no-label`.** Deferred.
  `key!=value` and `!key` forms are not part of v1.

## Risks and Open Questions

**Turn-time commands cost latency.** A label with `apply_on = "turn"` runs its
command before every request.
A slow command delays every turn, and v1 has no timeout to bound it.
The workaround is `apply_on = "new"`.

**Alias resolution depends on the merged config.** `--label=:name` resolves
against the configuration in effect for that invocation.
A conversation-scoped delta that drops the declaration makes the alias fail with
an unknown-alias error.

## Implementation Plan

**Phase 1.** Add the `labels` field to `Conversation`.
Add `conversation.labels` accepting static declarations only.
Add `--label` to `query` and `edit`, and `--no-label` to `edit`, and `--label`
filtering to `ls`.
Render labels in `conversation show`.
Relocate `CommandConfigOrString` and `ToolCommandConfig` to
`jp_config::types::command`, dropping the `Tool` prefix; no configuration
changes.

**Phase 2.** Add `cmd` declarations and parallel resolution.
Add the `apply_on` enum and turn-time re-resolution.
Add `:name` alias resolution and the label-key validator.
Add `--label` filtering to `grep`.

[RFD 040]: ../040-hidden-conversations-and-tool-context.md
