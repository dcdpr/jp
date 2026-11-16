# RFD 101: Conversation Labels

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-19
- **Extended by**: [RFD 103]
- **Summary**: Adds static, command-backed, and CLI-managed `key=value` labels
  to conversations for filtering and `jp c label` management.

## Summary

Conversations support one or more `key=value` labels stored in their metadata.
Labels can be configured under `conversation.labels.<name>` as static values or
external commands that run at conversation creation (and optionally on fork).
Users can also set, filter, and alias labels from the CLI, and manage them with
`jp c label` (`add`, `rm`, and `ls`).

## Motivation

[RFD 040] deferred general-purpose tagging as out of scope.
Users need a way to find conversations by the context in which they were
created, particularly the VCS branch (for example, finding every conversation
started on `feat-x`).

Three requirements drive the design:

1. Labels must support both manual assignment (`jp q --new --label=foo=bar`) and
   automatic application from configuration.
2. Automatic labeling must be VCS-agnostic: JP does not know about Git, but the
   user's workspace does.
3. Labels must integrate with config layering so projects, users, and workspaces
   can declare conventions independently.

Without first-class labels, users rely on title conventions or separate
workspaces per branch, neither of which composes.

This RFD covers the deterministic, config-driven parts of [#101].
LLM-driven auto-tagging is deferred to a follow-up RFD.

## Design

### User-facing behavior

#### Managing labels

`jp c label` manages conversation labels with one subcommand per operation:

```sh
jp c label add team=platform branch=main # add to the active conversation
jp c label add --id=jp-c17866928997 draft # add to a named conversation
jp c label rm team draft                  # remove by key
jp c label rm                             # remove every label
jp c label                                # list; `jp c label ls` is the same
```

Each mutation prints the modified labels and target conversation.
`rm` prints the removed `key=value` pairs so the output can be pasted back into
`jp c label add`.
Because label mutations are not recorded in the event stream (see
[Non-Goals](#non-goals)), this output is the only undo mechanism.

Label keys and `key=value` pairs are positional arguments, while the target
conversation is always passed via `--id` (accepted before or after the verb, so
`jp c label --id=X add k=v` and `jp c label add --id=X k=v` are equivalent).
Separating positional label arguments from `--id` prevents ambiguity: a label
key spelled like a conversation target (`active`, `pinned`, or a conversation
ID) cannot silently retarget the command.

Because each `key=value` pair is a single shell argument, values need no
escaping.
For example, `jp c label add branch=feat,exp` sets a single label whose value
contains a comma.

`add` also accepts `:name` to resolve a named rule from `conversation.labels`
(see [Aliases](#aliases)).
Aliases are rejected when `--id` targets multiple conversations, because a rule
resolves against a single conversation's effective config.

#### Setting labels at creation

`jp q` and `jp c fork` use a `--label` flag because their positional arguments
are already used for the query prompt and source conversation IDs:

> [!TIP]
> [RFD 103] removes `--label` and `--reset-labels` from both commands, leaving
> `jp c label` as the only way to mutate labels.
> Under a set-valued model, `--label` is ambiguous between adding and replacing
> values, and nothing reads labels during a turn, so running `jp c label`
> separately loses nothing.

```sh
jp q --new --label=team=platform --label=branch=main
jp c fork <id> --label=stage=review
jp c fork <id> --reset-labels --label=stage=fresh
```

`--label` is repeatable and takes one literal label per flag.
`jp q` accepts `:name` aliases; `jp c fork` does not, because it can fork
multiple source conversations at once.

By default, a fork inherits the source conversation's labels.
`jp c fork --reset-labels` clears all labels accumulated up to that point on the
command line, including inherited ones.
Flag order matters: `--label=a=1 --reset-labels` produces no labels, whereas
`--reset-labels --label=a=1` produces `a=1`.

`jp c edit` does not manage labels.
To label multiple conversations at once, run `jp c label add --id=+session
sprint=42`.

Bare labels (without `=`) are shorthand for `key=""`.
Filters treat a bare key as matching any value for that key.

#### Configured labels

Configured labels live under `conversation.labels.<name>`, where `<name>` is the
label key:

```toml
[conversation.labels]
team = "platform" # static, applied on new

[conversation.labels.branch]
value.cmd = { program = "git", args = ["rev-parse", "--abbrev-ref", "HEAD"] }
apply_on = { new = true, fork = true }

[conversation.labels.host]
value.cmd = "hostname --short"
run = "unattended"
```

When creating a conversation, JP resolves every entry with `apply_on.new =
true`:

- A static `value` is used as-is.
- A command `value` runs the program in the workspace root and uses its trimmed
  stdout as the label value.
- If a command fails, JP logs a warning, skips that label, and still creates the
  conversation.

#### CLI directive semantics

Configured labels resolve first, and CLI directives apply on top from left to
right (the last value for a given key wins):

```sh
jp q --new --label=branch=main --label=branch=feat # branch=feat
jp c label add branch=main branch=feat             # branch=feat
```

A CLI label directive mutates conversation metadata directly; it does not merge
into `PartialAppConfig` or emit a `ConfigDelta` for `conversation.labels`.
`conversation.labels` defines rules for producing labels, whereas `--label`
assigns a resolved value to a conversation.
Unlike `--model` (which is shorthand for `--cfg assistant.model`), `--label` has
no `--cfg` equivalent.
To override a label rule from the CLI, use `--cfg
conversation.labels.<key>.value=...`.

#### Persistence on existing conversations

Every CLI directive writes directly to `metadata.json.labels` via
`ConversationMut::update_metadata` while holding the conversation lock, without
invoking the config pipeline or emitting a `ConfigDelta`.

#### Label removal

`jp c label rm <key>...` removes the named keys, and a bare `jp c label rm`
removes all labels.
Both mutate `metadata.json` directly without emitting a `ConfigDelta`.

Because positional arguments on `jp c label rm` are always label keys and the
target conversation is always `--id`, a bare `jp c label rm` is unambiguous.
An optional-value removal flag on other commands would compete with positional
arguments, so removal is only supported on `jp c label rm`.

Removing a key that is not present on the conversation succeeds and prints a
warning: `⚠ Conversation <id> has no label '<key>'; nothing to remove.`.
Idempotent removal lets scripts run `jp c label rm draft` without checking
whether `draft` is set, while the warning surfaces mistyped keys or unintended
target conversations (matching `jp c unarchive` on an unarchived conversation,
unlike `--tool=<unknown>`, which errors because the tool is missing from
configuration).

`jp q` has no label-removal flag because stripping metadata as a side effect of
a query turn is surprising.
`jp c fork` provides `--reset-labels` instead of per-key removal because forks
typically drop the entire inherited label set.

Removing a label only updates the conversation's stored metadata, not the
configured rules.
If a rule has `apply_on.new = true`, its label will still be applied to future
conversations, but it will not be re-added to the existing conversation (see
[Existing-conversation mutation](#existing-conversation-mutation)).

#### Filtering

`ls` and `grep` accept repeatable `--label` filters using `kubectl` semantics:
multiple flags combine with logical AND, `key=value` requires an exact value
match, and `key` matches any conversation where `key` is present.

```sh
jp c ls --label=branch=main --label=team
jp c grep --label=team=platform 'error'
```

#### Aliases

A `:name` directive resolves the configured `conversation.labels.<name>` entry
to its `key=value` pair.
Both static and command-backed entries can be referenced as aliases; alias
resolution uses the same resolver and `run` policy as automatic application (see
[Resolution](#resolution)).

```sh
jp q --new --label=:branch # adds branch=<git rev-parse output>
jp c label add :branch     # the same, on an existing conversation
```

Aliases resolve independently of automatic application.
If an entry has `apply_on.new = true` and the user also passes `--label=:name`,
JP resolves the entry twice (prompting twice under `run = "ask"`, or running the
command twice under `run = "unattended"`), because the command may be
intentionally non-idempotent.

#### Alias scope

Aliases are accepted only when a command targets a single conversation: `q
--new`, `q --fork`, `q --id`, and `c label add` (with a single target).

Because a label rule resolves against one conversation's effective config and
JP's config pipeline builds a single resolved config per invocation, commands
that can target multiple conversations have no per-target config to resolve
against.
Accordingly, `jp c label add` rejects aliases when `--id` matches multiple
conversations, and `jp c fork` rejects aliases at parse time.

Filter commands (`ls`, `grep`) also reject `:alias` with an error pointing to
the `key` / `key=value` syntax, because filters match persisted metadata rather
than configured rules.

#### Display

`jp c label` (or `jp c label ls`) prints a conversation's labels, and `jp c
show` renders them in the metadata block.
`jp c ls` omits labels to keep the table narrow; a `--label` column flag can be
added later if needed.
Labels are persisted in the conversation directory's `metadata.json`.

### Source of truth

Label rules and resolved labels live in two separate stores:

- **`conversation.labels` (config)**: unresolved rules for producing label
  values (static string, command, and `apply_on` policy), merged across the
  normal configuration layers.
- **`metadata.json.labels` (resolved)**: the conversation's current label set as
  a `BTreeMap<String, String>`.
  Filters, `jp c show`, and future tool integrations read this map.

The resolver computes the label map from configured rules, inherited source
labels (on fork), and CLI directives at three points:

1. **Conversation creation** (`jp q --new`): resolves every configured entry
   with `apply_on.new = true`, then applies CLI directives on top (see
   [Resolution](#resolution)).
2. **Fork** (`jp c fork`): copies the source conversation's labels, re-resolves
   configured entries with `apply_on.fork = true` on top, and then applies CLI
   directives.
3. **Existing-conversation mutation** (`jp q --id --label`, `jp c label`):
   updates only the keys named on the CLI without re-resolving other configured
   labels.
   Literal directives apply directly without spawning commands or prompting,
   while alias directives run the full resolver (including command execution and
   `run` prompts).
   The updated map is written directly to `metadata.json` with no `ConfigDelta`
   (see [Existing-conversation mutation](#existing-conversation-mutation)).

At creation and fork, resolved rules overwrite matching keys, while mutations on
existing conversations only touch keys named on the CLI.
Metadata changes never propagate back to configuration.

### Data model

> [!TIP]
> [RFD 103] replaces this map with a set of values per key so `crate=jp_config`
> and `crate=jp_llm` can coexist, preserving insertion order and treating
> single-valued labels as single-element sets.

```rust
// jp_conversation::Conversation
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub labels: BTreeMap<String, String>,
```

A missing `labels` field deserializes as an empty map, so existing conversations
require no migration.

Label keys must match `[A-Za-z][A-Za-z0-9_-]*` (an ASCII letter followed by
ASCII letters, digits, underscores, or hyphens).
Disallowed characters have syntactic meaning elsewhere: `.` separates paths in
`ConfigDelta` keys (`conversation.labels.<key>`), `=` separates keys from
values, `:` prefixes aliases, whitespace splits shell arguments, and a leading
`-` would be parsed as a CLI flag.
Malformed keys are rejected during config loading and CLI parsing.

Label values can contain any character except line breaks, including commas and
equals signs.

> [!TIP]
> Label values cannot contain line breaks because every output surface formats
> each label on a single line (`key=value` lines in `jp conversation label` and
> `jp conversation ls`, and `ID:TURN:SCOPE:KIND:TEXT` records in `jp
> conversation grep`).
> `Labels` replaces each `\n`, `\r\n`, and `\r` with a single space on load and
> store, normalizing both hand-edited `metadata.json` files and JP-written
> values (so values that differ only in line breaks collapse into one).
>
> `--label` filters and `jp conversation label rm` normalize line breaks the
> same way when matching values.

### Config shape

The `jp_config::conversation::label` module mirrors `conversation::tool`.
`ConversationConfig::labels` is a `MergeableMap<LabelConfig>`, supporting the
standard map merge strategies (`deep_merge`, `merge`, `keep`, `replace`) across
config layers.

```rust
pub struct ConversationConfig {
    // ... existing fields ...
    #[setting(nested, merge = map_with_strategy)]
    pub labels: MergeableMap<LabelConfig>,
}

#[serde(untagged)]
pub enum LabelConfig {
    /// Shorthand: `foo = "bar"` — a static label value with default `apply_on`
    /// and `run`.
    Static(String),

    /// Full form: `foo = { value, apply_on, run }`.
    Object(LabelObject),
}

pub struct LabelObject {
    /// The label's value: a literal string, or a command whose stdout produces
    /// the value at resolution time.
    #[setting(default = "")]
    pub value: LabelValue,

    /// When this label is auto-applied.
    /// Independent of CLI / alias use.
    #[setting(default)]
    pub apply_on: ApplyOn,

    /// Confirmation policy for command-shaped values.
    /// Ignored for `Static` values.
    /// Defaults to `Ask`.
    /// A label-specific enum; conceptually similar to plugin `RunPolicy` (see
    /// [RFD 077]), not shared with tool `RunMode` (which has different
    /// variants).
    #[setting(default)]
    pub run: LabelRunMode,
}

#[serde(untagged)]
pub enum LabelValue {
    /// Static value: `value = "foo"`.
    Static(String),

    /// Command: `value.cmd = "..."` (shell-split string shorthand) or
    /// `value.cmd = { program, args, shell }` (structured).
    Command { cmd: CommandConfigOrString },
}

#[derive(Default)]
pub struct ApplyOn {
    /// Resolve and apply when a new conversation is created (`jp q --new`).
    /// Default: `true`.
    #[setting(default = true)]
    pub new: bool,

    /// Re-resolve and apply when an existing conversation is forked (`jp c
    /// fork`).
    /// Default: `false`.
    /// When `false`, the source conversation's existing value (if any) is
    /// inherited verbatim.
    #[setting(default)]
    pub fork: bool,
}

pub enum LabelRunMode { Ask, Unattended, Deny }
```

In TOML, a string (`labels.foo = "bar"`) deserializes as `LabelConfig::Static`,
and a table (`value`, `apply_on`, or `run`) deserializes as
`LabelConfig::Object`.
Inside `LabelObject`, a string `value` is static and `value.cmd = ...` is a
command, avoiding ambiguity between a static string and the string shorthand of
`CommandConfigOrString`.

| TOML                                                                   | Resolved label                                  |
| ---------------------------------------------------------------------- | ----------------------------------------------- |
| `labels.foo = "bar"`                                                   | `foo=bar` (static, `apply_on = { new = true }`) |
| `labels.foo = ""`                                                      | `foo=` (bare)                                   |
| `labels.foo = { value = "x" }`                                         | `foo=x` (static, defaults)                      |
| `labels.foo = { value = "x", apply_on = { new = true, fork = true } }` | `foo=x`, applied on new and fork                |
| `labels.foo = { value.cmd = "git rev-parse ..." }`                     | `foo=<stdout>` (command, shell-split string)    |
| `labels.foo = { value.cmd = { program = "git", args = ["..."] } }`     | `foo=<stdout>` (command, structured)            |
| `labels.foo = { value.cmd = "...", run = "unattended" }`               | command, no prompt                              |

### `CommandConfig`

`CommandConfigOrString` and `CommandConfig` live in
`crates/jp_config/src/types/command.rs` (extracted from `conversation/tool.rs`;
see [ubiquitous-language: CommandConfig][cmd-cfg]).
The string shorthand (`command = "git log --oneline"`) is parsed with
`shlex::split`:

- `"echo 'hello world'"` parses to a single `hello world` argument.
- Unbalanced quotes are rejected at config parse time by
  `PartialCommandConfigOrString::from_str`.

Label configuration reuses `CommandConfigOrString` with its existing `program`,
`args`, and `shell` fields.
Execution confirmation is defined per consumer rather than on `CommandConfig`
itself: tools use `ToolConfig::run`, and labels use `LabelObject::run` (see
[Resolution](#resolution)).
A command-backed label without `run = "unattended"` prompts before each
execution, including when `shell = true`.

### Resolution

Label resolution lives in `jp_cli` (or a dedicated `jp_label` crate) rather than
`jp_workspace`, which only manages storage and locking:

- `jp_config` defines the config types, validation, defaults, and merge
  strategies.
- `jp_cli` (or `jp_label`) executes commands, prompts according to `run`, and
  builds the resolved `BTreeMap<String, String>`.
- `jp_workspace` receives the resolved map and persists it via
  `ConversationMut::update_metadata`.

The caller resolves labels before creating the conversation:

```rust
let resolved = label::resolve(&config, &cwd, &approval_ctx).await?;
ws.create_and_lock_conversation(
    Conversation { labels: resolved, ..conv },
    base_config,
    session,
)?;
```

Resolution runs in six steps:

1. Filter `conversation.labels` to entries with `apply_on.new = true` (or
   `apply_on.fork = true` on fork).
2. Resolve static entries directly.
3. Check `run` for command-backed entries:
   - `Ask`: if a TTY is available, prompt the user with the rendered command and
     omit the label if rejected.
     Without a TTY, abort before creating the conversation and instruct the user
     to set `run = "unattended"` or `run = "deny"`.
   - `Unattended`: run without prompting.
   - `Deny`: skip the entry and omit the label.
4. Run all approved commands in parallel in the workspace root (with no timeout
   in v1) and trim their stdout to produce each label value.
5. If a command fails to spawn or exits non-zero, log a warning and skip the
   entry.
6. Apply CLI `--label` directives from left to right on top of the resolved map
   (see [User-facing behavior](#user-facing-behavior)).

#### Failure semantics

Steps 3 to 5 apply to automatic resolution (`apply_on.new` and `apply_on.fork`),
where prompt rejection, `run = "deny"`, spawn errors, and non-zero exits log a
warning, omit the label, and let the command continue.
Explicit alias directives (`--label=:name`) fail strictly: an unknown alias,
`run = "deny"`, a spawn error, or a non-zero exit returns an error and leaves
metadata unchanged.
Interactive rejection at the `Ask` prompt is the only exception: because the
user explicitly declined the command, JP logs a warning, omits the alias, and
continues.

#### Fork

Forking clones the source conversation's labels into the new conversation,
re-resolves configured entries with `apply_on.fork = true` on top, and then
applies CLI directives (`--label` adds values, and `--reset-labels` clears all
labels accumulated up to that point, including inherited ones).
`jp c fork` accepts multiple source conversations and therefore allows only
literal directives; `jp q --fork` forks a single source conversation and
resolves aliases against that source's configuration.

#### Existing-conversation mutation

`jp q --id --label` and `jp c label` load `metadata.json.labels`, apply the CLI
directives from left to right, and write the updated map back to `metadata.json`
under the conversation lock without emitting a `ConfigDelta` or re-evaluating
`apply_on` rules.

Literal directives update the map directly without spawning commands or
prompting.
Alias directives resolve the named `conversation.labels` entry through the
resolver (including command execution and `run` prompts) and write the resulting
`key=value` to the conversation.

#### Multi-target edits

When `jp c label` targets multiple conversations via `--id`, it applies literal
directives to each conversation under its respective lock.
Aliases are rejected whenever `--id` resolves to more than one conversation (see
[Alias scope](#alias-scope)).

To refresh a command-backed label on an existing conversation, run `jp c label
add :name`, edit the config, or fork the conversation.

Precedence from highest to lowest:

```text
CLI directives (applied left-to-right; last directive wins per key)
  > re-resolved configured labels (apply_on.fork on fork, apply_on.new on new)
  > inherited source-conversation labels (fork only)
```

## Drawbacks

- **Conversation-create latency.** Resolving command-backed labels spawns
  subprocesses on every `jp q --new`.
  Fast commands such as `git rev-parse` add negligible latency, and approved
  commands run in parallel, but a slow command still blocks conversation
  creation because v1 has no command timeout.

- **Persisted command output may be committed.** Resolved label values are
  written to `metadata.json`, which [RFD 031] projects into workspace storage
  for non-local conversations.
  Declaring `host = { value.cmd = "hostname" }` in workspace config leaks the
  local hostname into committed conversation metadata.
  Users should keep sensitive command-backed labels in user-global or
  user-workspace config, or use `--local` conversations.

- **No type-level enforcement of `run` policy.** `CommandConfig` does not
  enforce confirmation at the type level, so a future caller could execute a
  `CommandConfig` without checking a `run` policy.
  For v1, each consumer (`ToolConfig` and `LabelObject`) carries its own `run`
  field; an `execute(policy)` API on `CommandConfig` is deferred to future work.

- **Duplicate execution when combining alias and auto-apply.** If an entry has
  `apply_on.new = true` and the user also passes `:name`, the command runs once
  for automatic application and once for the alias.
  Users who want the command to run only on explicit request can set
  `apply_on.new = false`.

## Alternatives

### Array-of-tables for label config

Use `[[conversation.labels]]` entries with a `name` field, matching
`conversation.attachments`.
Rejected because all other named configurations (`conversation.tools.<name>`,
`providers.llm.<name>`, `plugins.command.<name>`) use map keys, which enforce
key uniqueness and support direct config delta paths.

### `run` field on `CommandConfig`

Attach a confirmation policy (`run = "ask" | "unattended"`) directly to
`CommandConfig` so every caller inherits it.
Rejected because confirmation policy belongs to the consumer, not the command
shape: two consumers may apply different trust policies to the same command, and
`command.run` would create layering conflicts with `tool.run`.
Placing `run` on `LabelObject` matches `ToolConfig::run`.

### Label management through flags on `jp c edit`

Manage labels with `--label` and `--no-label` flags on `jp c label` and `jp c
edit`, taking the conversation target as a positional argument and splitting
flag values on commas.

Rejected because optional flag values and positional conversation targets
compete for the same token.
In `jp c label --no-label active`, `active` binds as the `--no-label` key,
leaving no target argument and silently retargeting the active conversation.
Because conversation IDs and target keywords are valid label keys, syntax
validation cannot disambiguate them.
Comma-splitting also makes literal commas in values require an escape flag
(`--raw-label`).

Using `add`, `rm`, and `ls` subcommands with positional label arguments and
`--id` for the target conversation eliminates both problems (see [Managing
labels](#managing-labels)).

### Multi-key command output when `key` is omitted

In an earlier draft using array-of-tables, omitting `key` on a command entry
parsed stdout as `KEY=VALUE` lines.
Rejected because forgetting `key` on a single-value command silently produced no
labels when stdout contained no `=`.
Map-keyed configuration provides the key from the TOML map key, and multi-key
command output is omitted from v1.

### Plugin-event hooks for label production

A future plugin event-subscription system could let plugins emit labels on
`conversation_created` via the `ConversationLock` write API.
Deferred until plugin event subscriptions exist.

### Bare labels as a distinct type

Store bare labels in a separate `BTreeSet<String>` alongside `BTreeMap<String,
String>`.
Rejected because TOML has no `null` literal, a separate set requires a second
filter representation, and `value = ""` represents bare labels without a second
type (matching `kubectl`).

### Turn-time label refresh

Add `apply_on = "turn"` to re-resolve labels at the start of every turn before
sending the LLM request.
Rejected because neither the LLM prompt nor `jp_tool::Context` reads
conversation labels (exposing `Context.labels` is a non-goal).
Refreshing labels at turn start would only affect metadata read by later `show`,
`ls`, or `grep` commands, which does not justify the subprocess latency or
failure-handling complexity on every turn.

## Non-Goals

- **Multi-key command output.** Each command entry produces a single label value
  in v1.
- **`Context.labels` exposure to tools.** Tools do not receive conversation
  labels until an explicit opt-in is designed, because labels may contain
  sensitive values.
- **Label-value change history.** Label mutations write directly to
  `metadata.json` without emitting event-stream records, so only the current
  label set is stored.
  Changes to configured rules (`conversation.labels` via `--cfg` or config
  files) still emit `ConfigDelta` events like any other config change.
- **Negative filters.** No `--label=!foo` or `--label=foo!=bar`; filters support
  only conjunctive positive matches.
- **Cardinality limits.** No hard limit on label count or value length, though
  conversations are expected to carry a small number of short `key=value` pairs.
- **Turn-time label refresh.** Covered in [Turn-time label
  refresh](#turn-time-label-refresh).
- **Per-target alias resolution.** Because a single CLI invocation builds one
  resolved `AppConfig`, aliases are restricted to single-target commands rather
  than building a per-target config pipeline.

## Risks and Open Questions

- **Public surface stability.** The `labels` field in `metadata.json`, the `jp c
  label` subcommands, the `--label` and `--reset-labels` flags, the `:alias`
  syntax, the `apply_on` config schema, and `jp c show` rendering all become
  part of the public interface once Phase 1 merges.

- **`jp c l` alias reassignment.** `jp c l` changes from an alias for `jp c ls`
  to an alias for `jp c label` (`jp c ls` retains `list`).
  Label management is frequent enough in interactive use to claim `l`, while
  `ls` is already two characters.

- **Three-level subcommands.** `jp c label add` is JP's first three-level
  subcommand (`jp a`, `jp config`, `jp c`, and `jp plugin` are two-level).
  The extra level keeps positional arguments dedicated to label keys and pairs.

- **Alias resolution and config layering.** `:alias` resolves against the merged
  `AppConfig` for the invocation, which layers at most one conversation's config
  (selected via `ConversationLoadRequest::config_conversation`).
  Confining aliases to single-target invocations avoids rebuilding `AppConfig`
  per target (see [Alias scope](#alias-scope)).

- **`apply_on.fork` and source conversation config deltas.** `jp c fork` does
  not layer per-conversation config, so a label rule added only via a
  `ConfigDelta` on the source conversation is not visible to `apply_on.fork`
  during `jp c fork` (whereas `jp q --fork` layers the source conversation's
  config and sees the rule).
  This discrepancy will disappear if `jp c fork` is restricted to a single
  source conversation.

- **Working directory for label commands.** Label commands run in the workspace
  root rather than the user's current working directory, matching
  `attachment_cmd_output` and keeping command execution deterministic across
  subdirectories.

## Implementation Plan

### Phase 1: data model, static labels, basic CLI

Mergeable independently.

1. Add `Conversation::labels: BTreeMap<String, String>` to `jp_conversation`
   with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`.
2. Add `jp_config::conversation::label` with `LabelConfig` (`Static` and
   `Object` variants) and wire `labels: MergeableMap<LabelConfig>` into
   `ConversationConfig` with `map_with_strategy`.
   In Phase 1, `apply_on` and `run` are parsed but inactive, and command-backed
   values (`value.cmd = ...`) are rejected.
3. Add `jp c label` (`add`, `rm`, and `ls`) taking keys and `key=value` pairs as
   positional arguments and the target conversation via `--id`.
   A bare `jp c label` lists labels; a bare `jp c label rm` removes all labels.
   Print the affected conversation and labels on every mutation, including the
   removed `key=value` pairs on `rm`.
   Add repeatable literal `--label` flags to `query` and `conversation fork`,
   and `--label` filtering to `ls`.
   Apply directives from left to right (last value wins per key) and validate
   keys against `[A-Za-z][A-Za-z0-9_-]*`.
   All mutations update `metadata.json` directly via
   `lock.as_mut().update_metadata(...)` under the conversation lock (`jp c
   label`, `q --new`, `q --id --label`, and `c fork --label`), without emitting
   a `ConfigDelta`.
4. Render labels in `jp c show`.
5. Reassign `jp c l` to `jp c label`, keeping `list` on `jp c ls`.

### Phase 2: command-backed labels, `apply_on`, `run` policy, aliasing, grep filter

Depends on Phase 1; mergeable independently.

1. Enable `apply_on` and `run` on `LabelObject`, and extend `LabelValue` to
   accept `value.cmd = "..."` and `value.cmd = { program, args, shell }`.
2. Implement label resolution in `jp_cli` (or `jp_label`) to run commands,
   prompt for `run = "ask"`, and build the resolved `BTreeMap<String, String>`
   passed to `Workspace::create_and_lock_conversation`.
3. Update fork to clone the source conversation's labels and re-resolve entries
   with `apply_on.fork = true` on top.
4. Parse `:name` into `LabelDirective::Alias(String)`.
   In `jp c label add`, load the target conversation's config, reject aliases
   when `--id` matches multiple conversations, and resolve aliases while holding
   the conversation lock.
   Reject `:alias` at parse time on `c fork`, `ls`, and `grep`.
5. Add `--label` filtering to `grep` by pre-filtering the target conversation
   set (leaving the `Scope` enum unchanged).
6. Add `--reset-labels` to `jp c fork`, evaluated in flag order alongside
   `--label`.

### Future work

- Expose `Context.labels` to tools behind an opt-in `expose_to_tools` setting.
- Add `apply_on.turn` once an in-turn consumer of label values exists.
- Support multi-key command output (`multi = true`).
- Enforce command confirmation policies at the type level (`execute(policy)`).
- Support plugin-emitted labels via plugin event subscriptions.
- Add LLM-driven auto-tagging (follow-up to [#101]).
- Support richer label key grammars (namespaced keys such as `team.platform` or
  Unicode), which require escaping in dotted `ConfigDelta` paths.
- Support negative label filters (`--label=!foo`, `--label=foo!=bar`).

## References

- [RFD 031]: Durable Conversation Storage with Workspace Projection
  (`metadata.json` is projected into workspace storage and visible to Git).
- [RFD 040]: Hidden Conversations and Tool Context (deferred general-purpose
  conversation tagging).
- [RFD 077]: Plugin Configuration and Trust Policy (defines plugin `RunPolicy`,
  which `LabelRunMode` mirrors).
- [`conversation.tools`][tools] config (structural template for
  `conversation.labels`).
- [#101]: Conversation tags feature (umbrella tracking issue).

[#101]: https://github.com/dcdpr/jp/issues/101
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 103]: 103-multi-value-conversation-labels.md
[cmd-cfg]: ../wiki/00dxybd-ubiquitous-language.md#commandconfig
[tools]: ../../crates/jp_config/src/conversation/tool.rs
