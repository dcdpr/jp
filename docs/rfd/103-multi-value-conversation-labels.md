# RFD 103: Multi-Value Conversation Labels

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-21
- **Extends**: [RFD 101]

## Summary

A conversation label holds a set of values rather than one: `crate=jp_config`
and `crate=jp_llm` coexist under the same key.
`jp c label` gains `set` alongside `add`, `rm` accepts `key=value` as well as
`key`, and filters match on set membership.
Label mutation moves entirely to `jp c label`; `--label` and `--reset-labels`
leave `jp query` and `jp conversation fork`.

## Motivation

[RFD 101] models a label as one value per key, following `kubectl`.
Real usage has outgrown that.

Labels are used to categorise a conversation along dimensions with a closed
vocabulary — which crate it touches, which client, which review stage.
Dimensions are naturally multi-valued: a conversation that changes shared logic
touches `jp_config` and `jp_llm`, and one that changes behaviour visible in more
than one frontend is about `web` and `macos` at once.
A single-valued key cannot say that.

Two workarounds exist today and both fail:

- **Encode the set in the value** (`crate=jp_config,jp_llm`).
  Filters compare the whole string, so `--label=crate=jp_llm` misses.
  Teaching the filter to split means the value *is* a set, stringly typed, with
  deduplication and ordering pushed onto the user.
- **One key per value** (`crate-jp_config`, `crate-jp_llm`).
  Filtering for one crate works, but "which crates does this conversation
  touch?" and "any conversation with a crate label" both need prefix scanning,
  and the grouping the producer already had is discarded and re-derived from a
  naming convention.

Doing nothing blocks the LLM-driven auto-tagging deferred by [RFD 101], which is
the case that surfaced this: a model asked to categorise a conversation against
a vocabulary must be able to pick more than one value per dimension.

## Design

### The model

A label maps a key to an ordered, deduplicated set of values.
A key never maps to an empty set: removing the last value removes the key.

Single-valued labels are the one-element case, not a separate concept.
There is no per-key cardinality declaration — see
[Alternatives](#per-key-cardinality).

### Managing labels

```sh
jp c label add crate=jp_config crate=jp_llm  # crate = {jp_config, jp_llm}
jp c label add crate=jp_cli                  # crate = {jp_config, jp_llm, jp_cli}
jp c label set crate=jp_cli                  # crate = {jp_cli}
jp c label rm crate=jp_llm                   # one value
jp c label rm crate                          # the whole key
jp c label rm                                # every label
```

`add` inserts into the key's set.
`set` replaces the key's set.
Both accept several keys in one invocation, and `set` replaces only the sets of
the keys it names — clearing every label is a bare `rm`.

`set` reports the values it displaced alongside the ones it applied, so a `set`
that overwrote more than the user expected can be undone from its own output.
That is the same property [RFD 101] gives `rm`, and it is why `set` is safe to
offer as a single command rather than forcing `rm` followed by `add`.

`set` exists as a verb rather than as `add --replace` because the flag form is
self-contradictory on its face, and because a verb keeps each name honest: `set`
goes on meaning what it means under [RFD 101], while `add` takes the behaviour
that is genuinely new.

`rm` accepts a bare key or a `key=value` pair.
[RFD 101] rejects `=` in a removal argument; that restriction is lifted, and a
key with an `=` is still invalid because the key grammar forbids it.

### Filtering

`key=value` matches when the key's set contains that value.
`key` matches when the key is present.

```sh
jp c ls --label=crate=jp_llm     # touches jp_llm, whatever else it touches
jp c ls --label=crate            # has any crate label
```

Set algebra — "exactly this set", "any of", "all of", negation — is out of
scope here and belongs to the conversation query DSL.

### Label mutation leaves `query` and `fork`

`--label` on `jp query` and `jp conversation fork`, and `--reset-labels` on `jp
conversation fork`, are removed.
`jp c label` becomes the only way to mutate labels.

Under a multi-valued model the flag has to mean either "add to the set" or
"replace the set", and neither reading is safe to fix in a flag that also
composes with configured rules: a rule with `apply_on.new` that produces
`branch=main` alongside `--label=branch=feat` accumulates to a two-value set
under one reading and silently discards the rule under the other.

The flag is not carrying semantic weight that a second command cannot.
Nothing reads labels during a turn — `Context.labels` is a Non-Goal in [RFD
101], and no code path consults them while a query runs — so labelling at
creation differs from labelling immediately after only in keystrokes:

```sh
jp q --new "..." && jp c label add crate=jp_config crate=jp_llm
```

`jp query --new` always activates the conversation it creates, so the second
command needs no `--id`.
`jp conversation fork --activate` gives the same for a fork; a non-activating
fork needs the fork's ID, which [RFD 050] Phase 1 prints.

Filters (`--label` on `jp c ls` and `jp c grep`) stay.
They read labels rather than writing them, so none of the ambiguity applies.

### Configured rules

A rule's value may be a list, and a command-backed rule emits one value per line
of stdout:

```toml
[conversation.labels]
team = "platform"

[conversation.labels.crate]
value = ["jp_config", "jp_llm"]

[conversation.labels.branch]
value.cmd = "git rev-parse --abbrev-ref HEAD"
```

Line-per-value composes with ordinary shell tools and needs no escaping rule; a
delimiter would reintroduce the quoting problem the CLI grammar was designed to
avoid.

This is distinct from the multi-*key* command output that [RFD 101] lists as
future work: a rule still produces values for exactly one key, and that key is
still the map key.

### Data model

```rust
// jp_conversation
pub struct Labels(IndexMap<String, IndexSet<String>>);
```

`IndexSet` rather than `BTreeSet`: value order is preserved and is part of the
contract.
Sorted order can be rendered from an ordered collection at any time, while
insertion order cannot be recovered from a sorted one, so preserving it is the
reversible choice.
The cost is that `metadata.json` is git-visible under [RFD 031], so two
conversations with the same values added in different orders produce different
bytes.
`conversation.attachments` already behaves that way.

The field is private and the invariant — no key maps to an empty set — is
enforced by the type:

| Method                     | Behaviour                                            |
| -------------------------- | ---------------------------------------------------- |
| `insert(key, value)`       | Adds to the key's set, creating it.                  |
| `set(key, values)`         | Replaces the key's set; empty input removes the key. |
| `remove_key(key)`          | Removes the key and returns its set.                 |
| `remove_value(key, value)` | Removes one value, dropping the key when it empties. |
| `get`, `contains`, `iter`  | Read access.                                         |

The invariant belongs on the map rather than on a non-empty set type: it is a
property of the collection, and a `NonEmptySet` would still let a caller insert
a key and then drain it.

`Labels` also owns the on-disk contract.
A value is read as either a scalar or an array, so conversations written before
this RFD load unchanged, and is always written as an array:

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

Accepting two shapes and emitting one means no migration step.

### Display and machine output

`jp c show` and `jp c label ls` render one row per key, values comma-separated.
The JSON form carries the set:

```json
{
  "key": "crate",
  "values": [
    "jp_config",
    "jp_llm"
  ]
}
```

This replaces the `{"key", "value"}` shape from [RFD 101].

## Drawbacks

- **A shipped surface is removed.** `--label` on `query` and `fork`, and
  `--reset-labels`, are described in [RFD 101] and implemented.
  Removing them costs a keystroke in the common interactive case and a second
  command in scripts.

- **Order-dependent bytes in committed metadata.** Preserving value order means
  the same logical label set can produce different `metadata.json` diffs
  depending on the order values were added.

- **Every label-shaped surface changes at once.** Data model, CLI grammar,
  filters, config, display, and JSON all move together; there is no useful
  intermediate state.

- **`add` changes meaning.** It replaces today and accumulates after this RFD.
  Pre-release, so no migration is owed, but the word does more work than it did.

## Alternatives

### Encode the set in the value

`crate=jp_config,jp_llm` as a single string.
Rejected: filters compare whole values, so drilling down needs the filter to
split the string, at which point the value is a set with no type behind it and
the user owns deduplication, ordering, and escaping.

### One key per value

`crate-jp_config=true` and `crate-jp_llm=true`.
Rejected: the dimension exists only as a naming convention, so "which crates?"
and "any crate?" require prefix scanning over keys, and the grouping the
producer already had is thrown away.

### Per-key cardinality

`conversation.labels.crate.multi = true`, leaving other keys single-valued.
Rejected on two counts.
Storage must hold sets regardless, so this is the same data-model change plus a
configuration knob.
More seriously, `add` would insert or replace depending on the target key's
configuration — the same command behaving differently according to data rather
than to what the user typed — and a key set from the CLI with no configured
rule would have no declared cardinality at all.

### `add --replace` instead of a `set` verb

Rejected: "add" and "replace" contradict each other in one invocation, and
offering both spellings for one operation runs against the consolidation [RFD
101] settled on for its flags.

### `BTreeSet` for values

Rejected: sorted storage gives identical bytes for identical sets, which is
better for committed metadata, but discards ordering permanently.
Order carries meaning in cases like primacy ("mostly `jp_config`, also touches
`jp_llm`"), and a sorted view can always be rendered from an ordered set.

### Keep `--label` and pick a semantic

Either accumulate or replace, documented.
Rejected: the choice is permanent once released, the flag saves one `&&`, and
its interaction with configured rules is surprising under both readings.
It can be reintroduced later if the two-command form proves annoying in practice
— adding a flag is cheap, removing one is not.

## Non-Goals

- **Set algebra in filters.** "Exactly this set", "any of", "all of", and
  negation belong to the conversation query DSL.
- **Declared value vocabularies.** Constraining `crate` to a known list of
  values is the follow-up this RFD unblocks, not part of it.
- **LLM-driven auto-labelling.** The [#101] follow-up, which depends on both
  this RFD and vocabularies.
- **Multi-key command output.** A rule still produces values for one key; the
  `multi = true` future-work item in [RFD 101] is unchanged.
- **Label change history.** Still no event-stream record; [RFD 101]'s Non-Goal
  stands.

## Risks and Open Questions

- **Hyrum's Law on value order.** Once order is preserved, something will depend
  on it.
  That is the intent, but it means sorting later would be a breaking change; the
  contract should say plainly that order is insertion order.

- **`Labels` is a new choke point.** Every label read and write goes through it,
  including `label::apply`, which currently manipulates the map directly.
  A missing method sends a caller looking for an escape hatch, which is how the
  empty-set invariant would erode.

## Implementation Plan

### Phase 1: data model

`Labels` in `jp_conversation` with the private field and the API above,
including scalar-or-array read and array write.
Replaces `BTreeMap<String, String>` on `Conversation`.
Callers move to the API.

Mergeable independently: the CLI keeps single-value behaviour by inserting into
one-element sets.

### Phase 2: CLI and filters

`set` as a verb, `rm key=value`, contains-semantics for filters, display and
JSON changes.
Removes `--label` from `jp query` and `jp conversation fork`, and
`--reset-labels` from `jp conversation fork`, along with the `LabelDirectives`
clap wrapper and its alias handling.

Depends on Phase 1.

### Phase 3: configured rules

List-valued `value`, and line-per-value stdout for command-backed rules.

Depends on Phase 1.
Independent of Phase 2.

### Future work (out of scope, future RFDs)

- Declared value vocabularies, with validation and ordered domains.
- LLM-driven auto-labelling against a vocabulary (the [#101] follow-up).
- Set algebra in filters, via the conversation query DSL.
- Reintroducing a creation-time label flag, if the two-command form proves
  annoying once [RFD 050] Phase 1 has landed.

## References

- [RFD 101]: Conversation Labels — the single-valued model this extends.
- [RFD 050]: Scripting Ergonomics for Conversation Management — Phase 1 prints
  the forked conversation's ID, so a script can label a fork it chose not to
  activate.
- [RFD 031]: Durable Conversation Storage with Workspace Projection —
  `metadata.json` is git-visible, which is what makes value ordering a
  diff-noise question.
- [#101]: Conversation tags — the umbrella issue whose auto-tagging half this
  unblocks.

[#101]: https://github.com/dcdpr/jp/issues/101
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
[RFD 101]: 101-conversation-labels.md
