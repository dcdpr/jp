# RFD 101: Conversation Labels

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-19
- **Extended by**: [RFD 103]

## Summary

Conversations gain one or more `key=value` labels, stored alongside their other
metadata.
Labels are configurable via `conversation.labels.<name>`, can be static or
produced by an external command at conversation creation (and optionally
re-resolved on fork), and are settable, filterable, and aliasable from the CLI.
`jp c label` owns label management, with `add`, `rm`, and `ls` verbs.

## Motivation

[RFD 040] deferred a general-purpose tagging system as out of scope.
The need has surfaced concretely: users want to find conversations by the
context in which they were created — most pressingly, the VCS branch.
"What conversations did I start while working on `feat-x`?" has no answer today.

Three requirements drive the design:

1. Labels must be both manually set (`jp q --new --label=foo=bar`) and
   automatically applied based on configuration.
2. Auto-labeling must be VCS-agnostic — JP doesn't know about Git, but a user's
   workspace does.
3. Labels must integrate with the existing config layering so that a project,
   user, or workspace can declare conventions independently.

Doing nothing leaves the gap in [RFD 040] open.
Users build ad-hoc workarounds (title conventions, separate workspaces per
branch) that don't compose.

This RFD covers the deterministic / config-driven parts of [#101]; LLM-driven
auto-tagging is deferred to a follow-up RFD.

## Design

### User-facing behavior

**Managing labels.** `jp c label` owns label management, with one verb per
operation:

```sh
jp c label add team=platform branch=main # add to the active conversation
jp c label add --id=jp-c17866928997 draft # add to a named conversation
jp c label rm team draft                  # remove by key
jp c label rm                             # remove every label
jp c label                                # list; `jp c label ls` is the same
```

Each mutation reports the labels it touched and the conversation they landed on.
A removal names the labels it actually took, values included, so the reported
line can be pasted back as an `add` — label mutations leave no event-stream
record (see [Non-Goals](#non-goals)), so the output is the only undo.

Keys and `key=value` pairs are **bare arguments**, so the shell splits them and
the conversation is never one of them.
The conversation is named with `--id`, accepted on either side of the verb: `jp
c label --id=X add k=v` and `jp c label add --id=X k=v` are the same command.

Separating the two vocabularies is what keeps the command unambiguous.
A label key and a conversation target would otherwise compete for the same
argument slot, and a key spelled like a target (`active`, `pinned`, a
conversation ID) would silently retarget the command.

Because a value is one whole argument, it needs no escaping: `jp c label add
branch=feat,exp` sets one label whose value contains a comma.

`add` accepts `:name` alongside literal pairs, resolving the named
`conversation.labels` rule (see [Aliases](#aliases)).
With more than one target it is rejected, because a rule resolves against one
conversation's effective config.

**Setting labels at creation.** `jp q` and `jp c fork` carry a `--label` flag,
because their argument slot is already taken by the query text and the source
conversation:

> [!TIP]
> [RFD 103] removes `--label` and `--reset-labels` from both commands, leaving
> `jp c label` as the only way to mutate labels.
> Under a set-valued model the flag has to mean either "add to" or "replace",
> and nothing reads labels during a turn, so the two-command form loses nothing
> but a keystroke.

```sh
jp q --new --label=team=platform --label=branch=main
jp c fork <id> --label=stage=review
jp c fork <id> --reset-labels --label=stage=fresh
```

`--label` is repeatable and takes one label per occurrence, taken literally.
`jp q` accepts `:name`; `jp c fork` does not, since it may fork several sources.

`jp c fork --reset-labels` drops every label accumulated so far, including the
ones inherited from the source.
It is positioned like any other directive, so `--label=a=1 --reset-labels` ends
with no labels and `--reset-labels --label=a=1` ends with one.
A fork inherits the source's labels by default.

Label management is deliberately absent from `jp c edit`.
Bulk labelling is `jp c label add --id=+session sprint=42`.

Bare labels (no `=`) are sugar for `key=""`.
Filter semantics treat them as "key present, any value."

**Configured labels** live under `conversation.labels.<name>`.
The map key is the label key:

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

At conversation creation, each entry with `apply_on.new = true` is resolved:

- Static `value` is taken as-is.
- Command-shaped `value` entries spawn the program at the workspace root; stdout
  (trimmed) becomes the label value.
- A failing command logs a warning and skips that label — the conversation is
  created regardless.

**CLI directive semantics.** Directives apply left to right, so the last value
wins when the same key appears more than once.
Configured labels are resolved first; CLI directives are applied on top.

```sh
jp q --new --label=branch=main --label=branch=feat # branch=feat
jp c label add branch=main branch=feat             # branch=feat
```

A CLI directive is a *metadata mutation*, not a config override.
It does not merge into `PartialAppConfig` and does not emit a `ConfigDelta`
against `conversation.labels`.
The distinction is deliberate: `conversation.labels` declares label *rules*
(what to produce, and when), while `--label` states *this conversation carries
this value*.
Unlike `--model` (shorthand for `--cfg assistant.model`), `--label` has no
config-key equivalent.
Users who want to declare a rule from the CLI use the generic config override:
`--cfg conversation.labels.<key>.value=...`.

**Persistence on existing conversations.** Every CLI directive writes directly
to `metadata.json.labels` via `ConversationMut::update_metadata`, under the
conversation lock.
No `ConfigDelta` is emitted and the config pipeline is not involved.

**Label removal.** `jp c label rm <key>...` removes named keys; a bare `jp c
label rm` clears every label.
Both are direct metadata mutations — no `ConfigDelta`, no negative-delta
machinery.

A bare `rm` is safe to spell that way because the argument slot holds only label
keys: an empty slot cannot swallow the conversation, which is always `--id`.
The same shape on a flag with an optional value would be ambiguous, which is why
it is not offered there.

Removing a key the conversation doesn't carry is not an error: removal is
idempotent, so a script can say `jp c label rm draft` to mean "ensure `draft` is
gone" without having to check first.
It is reported, though — `⚠ Conversation <id> has no label '<key>'; nothing to
remove.` — because a directive that did nothing usually means a mistyped key or
a command that targeted a different conversation than intended, and the message
names both.
This follows `jp c unarchive` on a conversation that isn't archived, which
reports and continues; it differs from `--tool=<unknown>`, which errors, because
that names something absent from *configuration* rather than requesting a state
that already holds.

Removal lives on `jp c label` only.
`jp q` has no removal flag: a query starts or continues a turn, and silently
stripping labels mid-turn is a surprising side effect of asking a question.
`jp c fork` has `--reset-labels` rather than keyed removal, because the thing a
fork wants to drop is the inherited set as a whole.

Removal affects the conversation's stored labels, not the rules that produced
them.
A label removed from a conversation whose config still declares it with
`apply_on.new` reappears on the *next* conversation created under that config.
It does not come back on the current one, because configured entries are not
re-resolved for existing conversations (see [Existing-conversation
mutation](#existing-conversation-mutation)).

**Filtering.** `ls` and `grep` accept `--label` filters with `kubectl`
semantics: AND across flags, exact match on `key=value`, presence match on `key`
alone.

```sh
jp c ls --label=branch=main --label=team
jp c grep --label=team=platform 'error'
```

**Aliases.** A configured label entry can be referenced as `:name`, resolving to
that entry's `key=value`.
Any configured label is alias-eligible, including command-backed ones — alias
resolution drives the same resolver that automatic application uses, and
inherits the same `run` policy (see [Resolution](#resolution)).

```sh
jp q --new --label=:branch # adds branch=<git rev-parse output>
jp c label add :branch     # the same, on an existing conversation
```

Aliases resolve independently of automatic application.
A label that has already been resolved via `apply_on.new` is *re-resolved* when
also requested via `--label=:name` — a second prompt under `run = "ask"`, a
second execution under `run = "unattended"`.
We do not dedupe across resolution sources, because the configured command may
be intentionally non-idempotent.

**Alias scope.** Aliases are accepted only where exactly one target conversation
is known: `q --new`, `q --fork`, `q --id`, and `c label add`.

A rule resolves against one conversation's effective config, and JP's config
pipeline produces a single resolved config per invocation, so there is no
per-target config to resolve against on a command that may target several
conversations.
`jp c label add` therefore rejects an alias when `--id` resolves to more than
one conversation, and `jp c fork` rejects it at parse time.

On filter commands (`ls`, `grep`), `:alias` is rejected with an error directing
the user to the resolved label syntax — filters operate on persisted label
values, not on configured entries.

**Display.** `jp c label` (equivalently `jp c label ls`) lists a conversation's
labels, and `jp c show` renders them under the metadata block.
`jp c ls` intentionally does not — the table is already wide for narrow
terminals; a future `--label` column flag can be added if it proves necessary.
The conversation directory's `metadata.json` carries the labels field.

### Source of truth

Two stores with distinct roles:

- **`conversation.labels` (config)** — the *unresolved declaration*: rules for
  producing label values (static string, command, `apply_on` policy).
  Layered through the normal config chain.
- **`metadata.json.labels` (resolved)** — the *current label set*: a plain
  `BTreeMap<String, String>` of resolved values.
  The view that filters, `jp c show`, and (future) tool exposure read.

The resolver derives the resolved set from the configured rules plus inherited
source-conversation labels (on fork) plus CLI directives.
It runs at three well-defined points:

1. **Conversation creation** (`jp q --new`): every configured entry with
   `apply_on.new = true` is resolved; CLI directives apply on top.
   Detailed in [Resolution](#resolution).
2. **Fork** (`jp c fork`): source labels are inherited, configured entries with
   `apply_on.fork = true` are re-resolved on top, then CLI directives apply.
3. **Existing-conversation mutation** (`jp q --id --label`, `jp c label`): only
   the keys named on the CLI are updated; unrelated configured labels are *not*
   re-resolved.
   Literal directives apply directly without spawning commands or invoking
   `run`-mode prompts; alias directives still go through the full resolver and
   may spawn commands and prompt per the configured `run` policy.
   The result is written straight to `metadata.json`; no `ConfigDelta` is
   emitted.
   Detailed in [Existing-conversation
   mutation](#existing-conversation-mutation).

If config and metadata disagree, creation and fork resolution overwrite the
resolved keys they process.
Existing-conversation label mutations only touch CLI-named keys; unrelated
metadata/config drift is left untouched.
There is no back-propagation from metadata to config.

### Data model

> [!TIP]
> [RFD 103] replaces this with a set of values per key, so `crate=jp_config` and
> `crate=jp_llm` can coexist.
> Single-valued labels become the one-element case, and value order is
> preserved.

```rust
// jp_conversation::Conversation
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub labels: BTreeMap<String, String>,
```

Missing field on load defaults to empty — old conversations migrate silently.
Label keys match the grammar `[A-Za-z][A-Za-z0-9_-]*`: an ASCII letter, followed
by any number of letters, digits, underscores, and hyphens.
The excluded characters are each significant somewhere the key is used: `.`
separates dotted `ConfigDelta` paths against `conversation.labels.<key>`, `=`
splits a key from its value, `:` marks an alias, and whitespace would break the
argument into two.
The leading character is narrower still, because keys are written as bare
command arguments and one starting with `-` would be read as a flag.
Validation rejects malformed keys at config load and CLI parse time.

Values carry no such restriction.
A value is one whole argument, so it may contain any character the shell can
pass through, commas and equals signs included.

### Config shape

A new module `jp_config::conversation::label` mirrors the shape of
`conversation::tool`.
The top-level field is `MergeableMap<LabelConfig>` so consumers can apply
standard merge strategies (`deep_merge`, `merge`, `keep`, `replace`) across
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

The single-string TOML form (`labels.foo = "bar"`) is unambiguously the static
value.
Any structured value (an object with `value`, `apply_on`, or `run`) uses the
`Object` form.
Within the `Object` form, `value` itself disambiguates between static and
command via the `cmd` key: a bare string is static, `value.cmd = ...` is a
command.
This avoids the string-or-command ambiguity that an untagged `Static | Command`
would otherwise create, where the string shorthand of `CommandConfigOrString`
would be unreachable for labels.

The shape table:

| TOML                                                                   | Resolved label                                  |
| ---------------------------------------------------------------------- | ----------------------------------------------- |
| `labels.foo = "bar"`                                                   | `foo=bar` (static, `apply_on = { new = true }`) |
| `labels.foo = ""`                                                      | `foo=` (bare)                                   |
| `labels.foo = { value = "x" }`                                         | `foo=x` (static, defaults)                      |
| `labels.foo = { value = "x", apply_on = { new = true, fork = true } }` | `foo=x`, applied on new and fork                |
| `labels.foo = { value.cmd = "git rev-parse ..." }`                     | `foo=<stdout>` (command, shell-split string)    |
| `labels.foo = { value.cmd = { program = "git", args = ["..."] } }`     | `foo=<stdout>` (command, structured)            |
| `labels.foo = { value.cmd = "...", run = "unattended" }`               | command, no prompt                              |

### `CommandConfig` (shared shape, already extracted)

`CommandConfigOrString` and its inner `CommandConfig` live in
`crates/jp_config/src/types/command.rs` (extracted from `conversation/tool.rs`
as a precursor to this RFD; see [ubiquitous-language: CommandConfig][cmd-cfg]).
The string-shorthand form (`command = "git log --oneline"`) is parsed with
`shlex::split`, so quoting is respected:

- `"echo 'hello world'"` parses to one `hello world` argument.
- Unbalanced quoting is rejected at config-parse time by
  `PartialCommandConfigOrString::from_str`.

The TOML field names (`program`, `args`, `shell`) are unchanged.
Label config consumes the type as-is.

The "`shell = true` implies confirmation" doc-note on the consumer-side shape
describes a tool-specific policy contract, not a property of the type itself.
Tool and label consumers each define their own `run` policy.
Whether the tool side actually enforces the `shell = true` contract today is a
separate concern, out of scope for this RFD.

Label-provider resolution applies its own per-entry `run` policy (see
[Resolution](#resolution)).
A label whose `value` is a shell-mode command without `run = "unattended"`
prompts the user before each execution.

### Resolution

Resolution is an imperative-shell concern.
It lives in `jp_cli` (alongside CLI flag parsing and approval prompting), not in
`jp_workspace` — the workspace crate has no process-execution dependency today
and intentionally owns storage and locking, not subprocess management.
The split is:

- `jp_config` owns the typed config shape and pure normalization (validation,
  defaults, merge strategies).
- `jp_cli` (or a small dedicated crate, e.g. `jp_label`) owns command execution,
  the `run`-mode prompt, and assembly of the resolved `BTreeMap<String,
  String>`.
- `jp_workspace` receives the already-resolved map and persists it via the
  existing `ConversationMut::update_metadata` API.

A resolver call looks roughly like:

```rust
let resolved = label::resolve(&config, &cwd, &approval_ctx).await?;
ws.create_and_lock_conversation(
    Conversation { labels: resolved, ..conv },
    base_config,
    session,
)?;
```

The resolution steps:

1. Iterate `conversation.labels` entries; filter to entries with `apply_on.new`
   (or `apply_on.fork` on fork).
2. Static entries resolve directly.
3. Command-shaped entries consult `run`:
   - `Ask`: in interactive mode (TTY available), prompt the user with the
     rendered command; on rejection, the label is omitted.
     With no TTY, resolution aborts with an error directing the user to set `run
     = "unattended"` or `run = "deny"` for the affected label; conversation
     creation is aborted and no partial metadata is written.
   - `Unattended`: execute without prompting.
   - `Deny`: skip; the label is omitted.
4. Approved commands run in parallel at the workspace root (no timeout in v1);
   capture stdout; trim; use as the value.
5. On failure (non-zero exit, spawn error), log a warning and skip the entry.
6. Apply CLI `--label` directives on top of the config-resolved set; last value
   wins for repeated keys (see [User-facing behavior](#user-facing-behavior)).

**Failure semantics.** Steps 3–5 describe **automatic-application** semantics:
rejection at the `Ask` prompt, denial via `run = "deny"`, spawn errors, and
non-zero exits all cause the entry to be omitted with a warning; the command
still succeeds.
**Explicit-alias resolution** (a `--label=:name` directive) uses stricter
semantics: a missing alias, a `run = "deny"` entry, a spawn failure, or a
non-zero exit all return an error and leave metadata unchanged — the user asked
for the label, and silently omitting it would be dishonest.
Interactive rejection at the `Ask` prompt is the exception: the user has just
told the terminal not to run the command, so the alias is omitted with a
warning, and the surrounding command continues.

**Fork.** When a conversation is forked, the source conversation's labels are
cloned into the new conversation as the starting point.
Configured entries with `apply_on.fork = true` are then re-resolved and override
the inherited values.
Finally, CLI directives apply on top: `--label` adds, and `--reset-labels` drops
everything accumulated to that point, including the inherited set.
`jp c fork` accepts multiple source conversations and takes literal directives
only; `jp q --fork` is a single-source path, so it accepts aliases and resolves
them against that source's config.

**Existing-conversation mutation.** `jp q --id --label` and the `jp c label`
verbs apply only to the keys named on the CLI: start from
`metadata.json.labels`, apply the directives in left-to-right order, and write
the result back to `metadata.json` under the conversation lock.
No `ConfigDelta` is emitted.
Unrelated configured labels are left untouched, and no `apply_on` filtering is
applied.

Literal directives bypass the resolver — no command spawn, no `run`-mode
prompt.
Alias directives are different: they resolve the named config entry through the
standard resolver (including command execution and the `run`-mode prompt) before
applying the resulting `key=value` to the conversation.
An alias on an existing conversation is conceptually "evaluate this configured
entry now, then apply its value as a mutation."

**Multi-target edits.** `jp c label` accepts several conversations via `--id`
and loops over them, applying its literal directives to each under that
conversation's lock.
Aliases are rejected when more than one conversation resolves (see [Alias
scope](#alias-scope)), so there is never per-target resolution to perform.

Refreshing a command-backed label (re-running its command) requires either `jp c
label add :name`, editing the config, or forking.

Precedence (most → least specific):

```text
CLI directives (applied left-to-right; last directive wins per key)
  > re-resolved configured labels (apply_on.fork on fork, apply_on.new on new)
  > inherited source-conversation labels (fork only)
```

## Drawbacks

- **Conversation-create critical path.** Resolving command-shaped labels spawns
  subprocesses on every `jp q --new`.
  For fast commands (`git rev-parse`) this is negligible; for slow ones it adds
  visible latency.
  Mitigated by parallel execution, but a deliberately slow command can still
  block creation. v1 ships without a timeout; a future revision may revisit.

- **Persisted command output may be committed.** Resolved label values land in
  `metadata.json`.
  Per [RFD 031], that file is projected into workspace storage for non-local
  conversations and is therefore visible to `git status` / commits.
  A `host = { value.cmd = "hostname" }` declared in workspace config will leak
  the local machine name into any committed conversation metadata.
  Mitigations: prefer `--local` conversations for sensitive sources, or declare
  such labels only in user-global / user-workspace config.

- **No type-level guarantee on command safety.** A future contributor could
  introduce `CommandConfig` somewhere new and forget to thread a `run` policy
  through it.
  Mitigated by per-consumer policy (label entries carry their own `run`) and
  review for now; a cleaner solution (an `execute(policy)` method that makes
  policy threading mandatory) is left for future work.

- **Alias + auto-apply on the same entry runs the command twice.** When a
  configured entry has `apply_on.new = true` and the user also names it with
  `:name`, the command runs once for auto-application and once for the alias.
  Documented, not a bug — users who want once-only resolution should set
  `apply_on.new = false` and rely on the alias alone.

## Alternatives

### Array-of-tables for label config

Use `[[conversation.labels]]` entries with a `name` field, matching the shape of
`conversation.attachments`.
Rejected because every other named-config in the codebase
(`conversation.tools.<name>`, `providers.llm.<name>`, `plugins.command.<name>`)
is map-style.
Diverging here makes the config language inconsistent for no gain.
Map-style also gives natural uniqueness and straightforward config delta
overrides.

### `run` field on `CommandConfig`

Attach a confirmation policy (`run = "ask" | "unattended"`) directly to
`CommandConfig` so any caller automatically inherits it.
Rejected at the *shape-type* level: confirmation is a property of the *use*, not
the command — two consumers can use the same command shape with different trust
postures, and a `command.run` would create layering ambiguity against
`tool.run`.
The right place for the policy is on the *consumer*.
This RFD puts `run` on `LabelObject` (the label consumer), consistent with how
`ToolConfig` carries its own `run` for tools.

### Label management through flags on `jp c edit`

Manage labels with `--label` and `--no-label` flags on `jp c label` and `jp c
edit`, taking the conversation as a positional argument, and comma-split the
flag values so several labels fit in one occurrence.

Rejected because it puts two vocabularies in one argument slot.
A positional is a conversation target, but `--no-label`'s optional value
competes for the same token, so `jp c label --no-label active` binds `active` as
a label key, leaves no target, and silently retargets the session's active
conversation.
The conversation-ID and target-keyword grammars are both subsets of the
label-key grammar, so no validation rule separates them cleanly — each attempt
closed one spelling and left the class open.

Comma-splitting compounded it: a value containing a comma becomes unwritable,
which needs an escape flag (`--raw-label`), which needs its own scope rules.

The verb design removes the collision instead of guarding it.
Keys are bare arguments, the conversation is always `--id`, and a value is one
whole argument, so splitting and its escape hatch are both unnecessary.
See [Managing labels](#user-facing-behavior).

### `key`-absence triggering multi-key cmd mode

In an earlier shape, omitting `key` on a cmd-shaped entry meant "parse stdout as
`KEY=VALUE` lines."
Rejected as a silent footgun: a user who forgets `key` on a single-cmd label
gets zero labels with no error.
Map-style instead gives `key` a natural default from the map name.
Multi-key mode is dropped from v1 entirely — write two entries.

### Plugin-event hooks for label production

A future plugin event-subscription mechanism could let a plugin emit labels on
`conversation_created`.
Deferred to a future RFD; v1 cannot depend on it.
Once that mechanism exists, plugin-emitted labels flow through the existing
`ConversationLock` write API without needing a new mechanism.

### Bare labels as a distinct type

Model bare labels as a `BTreeSet<String>` alongside `BTreeMap<String, String>`.
Rejected: TOML has no null, two filter syntaxes proliferate, and `value = ""`
covers the case unambiguously.
`kubectl` makes the same choice.

### Turn-time label refresh

Add `apply_on = "turn"`, re-resolving labels at every turn before the request is
sent to the LLM.
Rejected: no existing data path in JP exposes conversation metadata labels to
the LLM prompt or to `jp_tool::Context`; `Context.labels` is itself a Non-Goal.
A turn-start refresh would only affect persisted metadata read by later `show` /
`ls` / `grep` invocations, which doesn't justify the resolution cost or the
failure-semantic complexity.
A future RFD can revisit this once an observer (LLM context inclusion, tool
context exposure) is designed.

## Non-Goals

- **Multi-key cmd output.** A single cmd produces a single label value in v1.
- **`Context.labels` exposure to tools.** Tools do not see labels until an
  explicit opt-in is designed (labels may carry sensitive data).
- **Label-value change history.** Label mutations write straight to
  `metadata.json` and leave no event-stream record; only the current set is
  recoverable.
  Changes to the *rules* (`conversation.labels`, via `--cfg` or a config file
  edit) do land as `ConfigDelta` events like any other config change. v1 ships
  no label-specific event type, no label-change render, and no history UI.
- **Negative filters.** No `--label=!foo` or `--label=foo!=bar`.
  AND-of-match only.
- **Cardinality limits.** No hard cap on label count or value length.
  Soft expectation: short keys, short values, single-digit count per
  conversation.
- **Turn-time label refresh.** See the corresponding entry under
  [Alternatives](#turn-time-label-refresh).
- **Per-target alias resolution.** A single invocation resolves one config, so a
  command that may target several conversations cannot resolve a rule per
  target.
  Aliases are confined to single-target invocations instead of building a
  per-target config pipeline.
  Revisit if multi-target commands ever become per-target invocations.

## Risks and Open Questions

- **Hyrum's Law surface.** The on-disk `labels` field name, the `jp c label`
  verbs, the `--label` and `--reset-labels` flags, the `:alias` prefix, the
  `apply_on` field shape, and the rendering in `jp c show` all become part of
  the public contract once shipped.
  Validate the shapes before merging Phase 1.

- **`jp c l` changes meaning.** `l` was a visible alias for `jp c ls` and
  becomes one for `jp c label`.
  `jp c ls` keeps `list` as its alias.
  Accepted deliberately: label management is the more frequent interactive
  operation, and `ls` is short enough already.

- **Three-level subcommands are new.** `jp c label add` is the project's first;
  `jp a`, `jp config`, `jp c`, and `jp plugin` are all two-level.
  Accepted because the verb form is compact (`jp c label add foo=bar`) and
  because the grouping is what keeps the argument slot free for label keys.

- **Alias resolution and config layering.** `:alias` resolves against the merged
  config, not the workspace root config alone.
  `resolve_config` builds one `AppConfig` per invocation, layering at most one
  conversation's config (chosen by
  `ConversationLoadRequest::config_conversation`), so there is no per-target
  effective config at command-execution time.
  Aliases are therefore confined to single-target invocations rather than
  reshaping the pipeline to rebuild a config per target.
  See [Alias scope](#alias-scope).

- **`apply_on.fork` and the source conversation's config.** `jp c fork` layers
  no per-conversation config, so a rule that exists only as a config delta on
  the source conversation is invisible to `apply_on.fork` re-resolution there.
  `jp q --fork` layers the source's config and does see it.
  Accepted for now; the divergence disappears when `c fork` becomes
  single-source.

- **Workspace cwd vs. user cwd.** Cmd resolution runs at workspace root.
  A user invoking `jp q --new` from a subdirectory may expect commands to run
  there.
  Workspace root is the right default (deterministic, matches
  `attachment_cmd_output`); revisit if real usage disagrees.

## Implementation Plan

### Phase 1: data model, static labels, basic CLI

Mergeable independently.

1. Add `Conversation::labels: BTreeMap<String, String>` to `jp_conversation`.
   Default-empty serde.
2. Add `jp_config::conversation::label` module with `LabelConfig` accepting both
   `Static` and `Object` variants.
   The `Object` variant accepts the `value` field; `apply_on` and `run` are
   parsed but inactive (no resolver yet), and command-shaped `value` entries
   (`value.cmd = ...`) are rejected at this phase.
   Wire it into `ConversationConfig` as `MergeableMap<LabelConfig>` with
   `map_with_strategy` merge.
3. CLI: `jp c label` with `add`, `rm`, and `ls` verbs, taking keys and
   `key=value` pairs as bare arguments and the conversation as a global `--id`.
   A bare `jp c label` lists; a bare `jp c label rm` clears every label.
   Each mutation reports the labels and the conversation it touched; removals
   name the labels actually taken, values included.
   `--label` on `query` and `conversation fork`, repeatable, one label per
   occurrence, values taken literally; `--label` filter on `ls`.
   Directives applied left-to-right, last one wins per key.
   Label key validator enforcing `[A-Za-z][A-Za-z0-9_-]*`.
   Every path is a direct metadata mutation with no config-pipeline integration
   and no `ConfigDelta`:
   - `jp c label`: apply directives per target under the conversation lock, via
     `lock.as_mut().update_metadata(...)`.
   - `q --new` / `q --id --label`: apply directives inside `Query::run` after
     the lock is acquired, via `lock.as_mut().update_metadata(...)`.
   - `c fork --label`: apply directives inside `fork::run` after
     `fork_conversation` returns the new lock.
4. `jp c show` renders the labels block.
5. `jp c l` becomes an alias for `jp c label`; `jp c ls` keeps `list`.

### Phase 2: command-backed labels, `apply_on`, `run` policy, aliasing, grep filter

Mergeable independently of Phase 1, but depends on it.

1. Activate `apply_on` and `run` on the existing `Object` variant; extend
   `value: LabelValue` to accept command-shaped entries via the `cmd` key
   (`value.cmd = "..."` or `value.cmd = { program, args, shell }`).
2. Implement label resolution in `jp_cli` (or a new `jp_label` crate), driving
   command execution, the `run`-mode prompt, and assembly of the resolved
   `BTreeMap`.
   Pass the resolved map into `Workspace::create_and_lock_conversation`.
3. Wire fork: clone source labels, then re-resolve configured entries with
   `apply_on.fork = true` on top.
4. Parse `:name` into a `LabelDirective::Alias(String)` variant.
   `jp c label add` loads the target's per-conversation config so the rule
   resolves against it, and rejects an alias when `--id` resolves to more than
   one conversation.
   Reject `:alias` on `c fork` and on the filter commands with a descriptive
   error at parse time.
   Resolve aliases under the conversation lock, so a rule's command never runs
   for a conversation the command cannot go on to modify.
5. `--label` filter on `grep` (pre-filter on conversation set; `Scope` enum
   unchanged).
6. `jp c fork --reset-labels`, positioned among the `--label` flags.

### Future work (out of scope, future RFDs)

- `Context.labels` exposure to tools with an opt-in `expose_to_tools` flag.
- Turn-time label refresh (`apply_on.turn`), once an observer for label values
  inside a turn is designed.
- Multi-key cmd output (`multi = true`).
- `execute(policy)` type-level guarantee for command execution.
- Plugin-emitted labels via a future plugin event-subscription mechanism.
- LLM-driven auto-tagging (the [#101] follow-up).
- Richer label key grammars (namespaced keys like `team.platform`, Unicode) —
  requires escaping for dotted `ConfigDelta` paths, or a non-path-based mutation
  API for map entries.
- Negative filters (`--label=!foo`, `--label=foo!=bar`).

## References

- [RFD 031]: Durable Conversation Storage with Workspace Projection — workspace
  `metadata.json` is git-visible; the basis for the persistence drawback.
- [RFD 040]: Hidden Conversations and Tool Context — deferred general-purpose
  tagging; this RFD picks it up.
- [RFD 077]: Plugin Configuration and Trust Policy — broader trust model that
  this RFD's per-label `run` policy is consistent with; plugin `RunPolicy` is
  the closest neighbor in shape to `LabelRunMode`.
- [`conversation.tools`][tools] config — pattern this RFD mirrors for
  `conversation.labels`.
- [#101]: Conversation tags feature — the umbrella issue this RFD partly
  fulfills.

[#101]: https://github.com/dcdpr/jp/issues/101
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 103]: 103-multi-value-conversation-labels.md
[cmd-cfg]: ../architecture/ubiquitous-language.md#commandconfig
[tools]: ../../crates/jp_config/src/conversation/tool.rs
