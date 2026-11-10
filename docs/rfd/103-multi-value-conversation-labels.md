# RFD 103: Multi-Value Conversation Labels

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-21
- **Extends**: [RFD 101]
- **Summary**: Allows conversation label keys to hold sets of values, splits
  `set` from `add`, and filters by set membership.

## Summary

This RFD extends conversation labels so a single key can hold an ordered set of
values (such as `crate=jp_config` and `crate=jp_llm`).
`jp c label` separates `set` from `add`, `rm` accepts `key=value` in addition to
a bare `key`, filters match on set membership, and `--label` and
`--reset-labels` are removed from `jp query` and `jp conversation fork`.

## Motivation

[RFD 101] models a label as one value per key, following `kubectl`.
Labels categorize a conversation along dimensions with a closed vocabulary, such
as the crate it touches, the client, or the review stage.
Those dimensions are naturally multi-valued: a conversation that changes shared
logic touches both `jp_config` and `jp_llm`, and a change that affects multiple
frontends applies to both `web` and `macos`.
A single-valued key cannot express multiple values.

Two workarounds exist under the single-valued model, and both fail:

- **Encode the set in the value** (`crate=jp_config,jp_llm`).
  Filters compare the whole string, so `--label=crate=jp_llm` misses.
  If the filter splits the string, the value becomes a stringly-typed set that
  pushes deduplication and ordering onto the user.
- **One key per value** (`crate-jp_config`, `crate-jp_llm`).
  Filtering for a single crate works, but querying which crates a conversation
  touches or finding any conversation with a crate label requires prefix
  scanning across keys, discarding the key grouping in favor of a naming
  convention.

Single-valued keys also block the LLM-driven auto-tagging deferred by [RFD 101]:
a model categorizing a conversation against a vocabulary must be able to assign
more than one value per dimension.

## Design

### Model

A label maps a key to an ordered, deduplicated set of values.
A key never maps to an empty set: removing the last value removes the key.
Single-valued labels are simply one-element sets, with no per-key cardinality
declaration (see [Alternatives](#per-key-cardinality)).

### Managing labels

```sh
jp c label add crate=jp_config crate=jp_llm  # crate = {jp_config, jp_llm}
jp c label add crate=jp_cli                  # crate = {jp_config, jp_llm, jp_cli}
jp c label set crate=jp_cli                  # crate = {jp_cli}
jp c label rm crate=jp_llm                   # one value
jp c label rm crate                          # the whole key
jp c label rm                                # every label
```

`add` inserts values into a key's set, while `set` replaces the key's set.
Both subcommands accept multiple keys in one invocation.
`set` replaces only the keys passed to it; a bare `jp c label rm` clears every
label.

In [RFD 101], `set` is an alias for `add`.
Here, `set` keeps its replacement behavior while `add` changes from replacing to
accumulating.
Making `set` a separate verb avoids a self-contradictory `add --replace` flag.

`set` reports the displaced values alongside the newly applied values, so an
unintended overwrite can be undone from the command's own output, matching `rm`
in [RFD 101].

`rm` accepts either a bare `key` or a `key=value` pair.
[RFD 101] rejected `=` in removal arguments; lifting that restriction does not
allow `=` in key names, which the key grammar still forbids.

#### Argument resolution

All operands are resolved and grouped by key before any mutation is applied, so
passing the same key multiple times acts on the combined values rather than
applying each operand in sequence:

1. Parse every operand and expand each `:name` alias into the key and ordered
   values produced by its rule.
2. Group values by key in argument order, deduplicating repeated values within
   each key.
3. Apply the operation once per key: `add` inserts the grouped values, `set`
   replaces the key's set with them, and `rm` removes them.

Grouping before applying ensures that `jp c label set crate=jp_config
crate=jp_llm` sets `{jp_config, jp_llm}` rather than replacing the set twice and
leaving `{jp_llm}`.
Both `add` and `set` accept `:name` aliases, and an alias contributes all of its
values to its key's group.

### Filtering

A `key=value` filter matches when the key's set contains `value`.
A bare `key` filter matches when the key is present.

```sh
jp c ls --label=crate=jp_llm     # touches jp_llm, whatever else it touches
jp c ls --label=crate            # has any crate label
```

Repeating `--label` combines filters with logical AND, as in [RFD 101].
Specifying the same key twice therefore requires the key's set to contain both
values:

```sh
jp c ls --label=crate=jp_config --label=crate=jp_llm  # touches both
```

Set-expression syntax (exact-set matching, any-of, and negation) is out of scope
and belongs in the conversation query DSL.
Logical AND across repeated flags already covers "all of" matching without
dedicated syntax.

### Removing label mutation from `query` and `fork`

`--label` is removed from `jp query` and `jp conversation fork`, and
`--reset-labels` is removed from `jp conversation fork`.
`jp c label` is the only command that mutates labels.

In a multi-valued model, `--label` must either add to a key's set or replace it.
Neither choice composes cleanly with configured rules: if an `apply_on.new` rule
produces `branch=main`, passing `--label=branch=feat` either accumulates into a
two-value set (`{main, feat}`) or silently discards the rule.

Separating label mutation into a second command does not affect turn execution.
Nothing reads labels during a turn (`Context.labels` is a Non-Goal in [RFD 101],
and no code path inspects labels while a query runs), so when a turn succeeds,
labeling at creation and labeling immediately afterward produce the same result:

```sh
jp q --new "..." && jp c label add crate=jp_config crate=jp_llm
```

`jp query --new` always activates the conversation it creates, so `jp c label`
needs no `--id`.
`jp conversation fork --activate` behaves the same way; a fork without
`--activate` requires the new conversation ID printed by [RFD 050] Phase 1.

The two forms differ only when the turn fails.
`--label` applied before contacting the provider, so a missing credential or
dropped connection left a labeled conversation, whereas `&&` skips the second
command.
Configured rules (`apply_on.new`) still resolve during conversation creation, so
a failed turn loses only ad-hoc CLI labels (see [Drawbacks](#drawbacks)).

Read-only `--label` filters on `jp c ls` and `jp c grep` remain unchanged.

### Configured rules

A rule's `value` may be a list, and a command-backed rule emits one value per
line of stdout:

```toml
[conversation.labels]
team = "platform"

[conversation.labels.crate]
value = ["jp_config", "jp_llm"]

[conversation.labels.branch]
value.cmd = "git rev-parse --abbrev-ref HEAD"
```

Emitting one value per line composes with standard shell tools without escaping
rules; an inline delimiter would reintroduce the quoting ambiguity that the CLI
grammar avoids.

This differs from the multi-*key* command output listed as future work in [RFD
101]: a rule still produces values for a single key determined by its map key.

The accepted rule shapes extend [RFD 101]:

| TOML                                     | Resolved values           |
| ---------------------------------------- | ------------------------- |
| `crate = "jp_config"`                    | `{jp_config}`             |
| `crate = ["jp_config", "jp_llm"]`        | `{jp_config, jp_llm}`     |
| `crate = { value = "jp_config" }`        | `{jp_config}`             |
| `crate = { value = ["jp_config", ...] }` | the listed values         |
| `crate = { value = [] }`                 | no label                  |
| `crate = { value.cmd = "..." }`          | one value per stdout line |

A top-level array is shorthand for `value = [...]`, matching the bare-string
shorthand.
Across config layers, a rule's `value` replaces earlier layers rather than
appending to them: a workspace rule listing two crates and a user rule listing
one resolve to the single user crate.
Accumulating values across layers would leave no way to narrow an inherited
list, and the label map already uses `MergeableMap` strategies to merge entries
across keys.

Command stdout is split on newlines, replacing the whole-output trim in [RFD
101].
Empty lines are dropped, and a command that writes no non-empty lines produces
no label.
Retaining empty lines would turn a trailing newline into an empty string, which
represents a bare label.

**On fork, a matching rule replaces the key's set.** Forking starts from the
parent conversation's labels.
Each `apply_on.fork` rule then replaces the full set for its key, leaving keys
without a matching rule unchanged.
This preserves [RFD 101]'s rule that re-resolved entries override inherited
values: a conversation with `stage={draft, review}` forked under a rule
producing `{approved, ready}` receives `{approved, ready}`, not all four values.

When a matching rule resolves to no values (such as `value = []` or a command
with empty output), it removes the inherited key.
By contrast, if a rule's command fails, the error is reported and the inherited
values stand.
Rule resolution therefore distinguishes "matched with no values" from "did not
match", which a plain set of labels cannot express.

### Data model

```rust
// jp_conversation
pub struct Labels(BTreeMap<String, IndexSet<String>>);
```

Keys and values use different ordering guarantees:

- **`IndexSet` for values.** Insertion order is preserved as part of the
  contract.
  Order within a key can carry meaning (such as "primarily `jp_config`, also
  touches `jp_llm`"), and a sorted view can always be derived from an ordered
  set, whereas insertion order cannot be recovered once sorted.
  Because `metadata.json` is git-visible under [RFD 031], adding the same values
  in different orders produces different bytes on disk, matching the existing
  behavior of `conversation.attachments`.
- **`BTreeMap` for keys.** Key order carries no semantic weight, so preserving
  insertion order across keys would only add diff noise to committed metadata.
  Sorted keys also match what [RFD 101] writes.

The inner field is private, and `Labels` enforces two invariants:

**No key maps to an empty set.** This invariant belongs on the map rather than
on a `NonEmptySet` value type, because a `NonEmptySet` would still allow a
caller to insert a key and then drain its values.

**The empty string never coexists with a non-empty value.** A bare label (such
as `draft`) stores `""` to record that the key is present.
Once a key holds a non-empty value, the key's presence is already recorded,
making the `""` marker redundant.
Allowing both would let `jp c label add draft draft=urgent` produce `{"",
"urgent"}`, where `--label=draft=` matches and `jp c label rm draft=` mutates
the set even though display views show only `draft=urgent`.
Inserting a non-empty value removes `""`, inserting `""` into a key that already
holds a non-empty value is a no-op, and deserialization strips `""` when
non-empty values are present.

| Method                     | Behavior                                             |
| -------------------------- | ---------------------------------------------------- |
| `insert(key, value)`       | Adds to the key's set, creating it.                  |
| `set(key, values)`         | Replaces the key's set; empty input removes the key. |
| `remove_key(key)`          | Removes the key and returns its set.                 |
| `remove_value(key, value)` | Removes one value, dropping the key when it empties. |
| `get`, `contains`, `iter`  | Read access.                                         |

`Labels` also owns the on-disk format and deserializes through a validating
conversion rather than a derived `Deserialize`.
A derived implementation would populate the private map directly and allow
states the API forbids, such as `{"crate": []}` in a hand-edited `metadata.json`
([RFD 031]).
Deserialization normalizes invalid states rather than failing, so a minor manual
edit does not make the conversation unreadable:

- A scalar becomes a one-element set.
- An array is deduplicated, keeping the first occurrence of each value.
- An empty array drops the key, matching `set(key, empty)`.
- An array containing both `""` and non-empty values drops `""`.
- A non-string element returns a metadata load error.

Deserialization accepts either a scalar or an array so conversations written
under [RFD 101] load unchanged, while serialization always writes an array:

```json
{
  "labels": {
    "branch": [
      "main"
    ],
    "crate": [
      "jp_config",
      "jp_llm"
    ]
  }
}
```

Accepting both shapes on read and emitting arrays on write avoids a migration
step.
JP versions built before [RFD 101] ignore the unknown `labels` field regardless
of its value type.

### Display and machine output

`jp c label ls` prints one `key=value` pair per line, matching the CLI input
syntax:

```console
$ jp c label ls
 crate=jp_config
 crate=jp_llm
 draft
```

Joining a key's values with commas on a single line would be ambiguous because
[RFD 101] places no restriction on values: `branch=feat,exp` is a single valid
value, so `feat,exp, main` could be read as two values or three.

Every line begins with a single-character status column (` ` for `ls`).
Because label keys start with a letter, the leading column cannot be mistaken
for part of a key, and scripts can strip the first character regardless of which
subcommand printed the line.

Mutation subcommands format stdout as a diff: `-` for removed values, `+` for
added values, and ` ` for unchanged values on modified keys.
Per [RFD 048], the diff lines are data on stdout and the summary header goes to
stderr:

```console
$ jp c label rm crate=jp_llm
Removed labels from jp-c17866928997: Tool Chaining

 crate=jp_config
-crate=jp_llm
```

```console
$ jp c label set crate=jp_cli
Set labels on jp-c17866928997: Tool Chaining

-crate=jp_config
-crate=jp_llm
+crate=jp_cli
```

Because `set` prints displaced values with `-`, an overwritten label set can be
recovered directly from the command output.

`jp c show` renders one row per key and lists its values in a numbered
multi-line cell, matching the details view formatting for attachments and
compactions.

In JSON output, `jp c label ls` returns each key with its `values` array,
replacing the `{"key", "value"}` object from [RFD 101]:

```json
{
  "key": "crate",
  "values": [
    "jp_config",
    "jp_llm"
  ]
}
```

Mutation commands replace [RFD 101]'s flat `labels` array with a `changes` array
that records `before` and `after` value lists for each modified key:

```json
{
  "action": "set",
  "conversation": { "id": "jp-c17866928997", "title": "Tool Chaining" },
  "changes": [
    { "key": "crate", "before": ["jp_config", "jp_llm"], "after": ["jp_cli"] }
  ]
}
```

`add` and `rm` use the same schema, with one list being a subset of the other.
For a bare label, `before` and `after` are empty arrays (`[]`) rather than
exposing the internal `""` marker.

## Drawbacks

- **Removed CLI flags.** `--label` on `query` and `fork`, along with
  `--reset-labels` on `fork`, were implemented under [RFD 101].
  Removing them requires chaining a second command (`jp c label`) in interactive
  sessions and scripts.

- **Ad-hoc labeling is not atomic with `query --new`.** `jp q --new --label=x`
  wrote the label before contacting the provider, so the label survived a failed
  turn; `jp q --new && jp c label add x` does not run the second command when
  the query fails.
  A failed turn therefore leaves a conversation with its configured labels
  (`apply_on.new`) but none of its ad-hoc CLI labels.

- **Order-dependent bytes in committed metadata.** Preserving value insertion
  order means the same logical label set can produce different `metadata.json`
  diffs depending on the order values were added.
  Keys remain sorted, so this is confined to the values within a single key.

- **Broad surface change.** The data model, CLI grammar, filters, configuration
  rules, terminal display, and JSON schema all change together, with no useful
  intermediate state.

- **`add` changes behavior.** Under [RFD 101], `add` replaced the key's value;
  here it appends to the key's set.

## Alternatives

### Encode the set in the value

Store multiple values in a single comma-separated string, such as
`crate=jp_config,jp_llm`.
Rejected because filters compare whole values; filtering by element would
require splitting the string at query time, turning the value into an untyped
set where the user must manage deduplication, ordering, and escaping.

### One key per value

Encode each value as its own key, such as `crate-jp_config=true` and
`crate-jp_llm=true`.
Rejected because the dimension exists only as a naming convention: listing which
crates a conversation touches or matching any crate label requires prefix
scanning across keys, discarding the explicit key grouping.

### Per-key cardinality

Configure cardinality per key (such as `conversation.labels.crate.multi = true`)
while leaving other keys single-valued.
Rejected for two reasons.
First, storage must hold sets regardless, so this requires the same data-model
change plus a configuration setting.
Second, `jp c label add` would either insert or replace depending on the target
key's configuration, and ad-hoc keys set from the CLI without a configured rule
would have no declared cardinality.

### `add --replace` instead of a `set` verb

Rejected because `add --replace` is self-contradictory, and `set` is already
established by [RFD 101] as the subcommand for setting a key's value.

### `BTreeSet` for values

Rejected because sorting values in storage gives identical bytes for identical
sets in committed metadata, but discards insertion order permanently.
Order carries meaning in cases like primacy ("mostly `jp_config`, also touches
`jp_llm`"), and a sorted view can always be rendered from an `IndexSet`.

### `IndexMap` for keys

Rejected because preserving key insertion order would expose arbitrary ordering
differences in `metadata.json`, `jp c show`, and `jp c label ls`.
Unlike values within a key, keys have no relative priority, and [RFD 101]
already sorts keys.

### Keep `--label` and pick a semantic

Keep `--label` on `query` and `fork` and define it to either accumulate or
replace.
Rejected because the choice is permanent once released, the flag saves only a
second command (`&&`), and its interaction with configured rules is surprising
under both definitions.
A creation-time flag can be reintroduced later if the two-command workflow
proves awkward, whereas removing a released flag is a breaking change.

## Non-Goals

- **Set-expression filter syntax.** Exact-set matching, any-of, and negation
  belong to the conversation query DSL.
  ("All of" matching is already covered by repeating `--label`.)
- **Declared value vocabularies.** Constraining `crate` to a known list of
  values is a separate follow-up that this RFD unblocks.
- **LLM-driven auto-labeling.** Automatic labeling ([#101]) depends on both this
  RFD and declared vocabularies.
- **Multi-key command output.** A rule still produces values for a single key;
  the `multi = true` future-work item in [RFD 101] is unchanged.
- **Label change history.** Labels remain metadata with no event-stream record,
  as in [RFD 101].

## Risks and Open Questions

- **Hyrum's Law on value order.** Once insertion order is preserved, callers and
  scripts will depend on it.
  That is intentional, but it means switching to sorted storage later would be a
  breaking change; the contract must state explicitly that order is insertion
  order.

- **`Labels` as the single mutation boundary.** Every label read and write,
  including `label::apply`, must go through `Labels` rather than manipulating
  the underlying map directly.
  If the `Labels` API omits a needed operation, callers might reach for raw map
  access and bypass the non-empty and bare-label invariants.

## Implementation Plan

### Phase 1: Data model

Introduce `Labels` in `jp_conversation` with a private inner map, the mutation
and query methods above, scalar-or-array deserialization, and array
serialization.
Replace `BTreeMap<String, String>` on `Conversation` with `Labels` and move
callers to the new API.

This phase can merge independently if the CLI keeps replacing values: existing
`Set` directives must call `set(key, one_element_set(value))` rather than
`insert`.
Wiring up `insert` in Phase 1 would change `jp c label add` from replacing to
accumulating before Phase 2 introduces the `set` subcommand.

### Phase 2: CLI and filters

Add `set` as a subcommand, accept `key=value` in `rm`, switch `add` from `set`
to `insert`, update `--label` filters to check set membership, and update the
terminal and JSON output formats.

Remove `--label` from `jp query` and `jp conversation fork`, and remove
`--reset-labels` from `jp conversation fork`.
Delete the `LabelDirectives` clap wrapper along with its `ALIASES` switch (which
allowed `--label=:name` on `jp query` while rejecting it on `jp conversation
fork`) and its `RESET` switch (which registered `--reset-labels`).
Alias support on `jp c label` is unaffected because it goes through
`LabelDirective::parse_set` and `expand_aliases` rather than the wrapper.

This phase depends on Phase 1 and lands after [RFD 050] Phase 1, which prints
the forked conversation's ID so scripts can label a non-activated fork.

### Phase 3: Configured rules

Support list-valued `value` settings and split stdout on newlines for
command-backed rules.
This phase depends on Phase 1 and is independent of Phase 2.

### Future work (out of scope, future RFDs)

- Declared value vocabularies with validation and ordered domains.
- LLM-driven auto-labeling against a vocabulary (the [#101] follow-up).
- Set algebra in filters via the conversation query DSL.
- Reintroducing a creation-time label flag, if the two-command form proves
  awkward once [RFD 050] Phase 1 has landed.

## References

- [RFD 101]: Conversation Labels.
  Defines the single-valued label model that this RFD extends.
- [RFD 050]: Scripting Ergonomics for Conversation Management.
  Phase 1 prints the forked conversation's ID so a script can label a
  non-activated fork.
- [RFD 031]: Durable Conversation Storage with Workspace Projection.
  Makes `metadata.json` git-visible, which motivates deterministic key ordering.
- [#101]: Conversation tags.
  The umbrella issue whose auto-tagging work this RFD unblocks.

[#101]: https://github.com/dcdpr/jp/issues/101
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
[RFD 101]: 101-conversation-labels.md
