# RFD 103: Multi-Value Conversation Labels

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-21
- **Extends**: [RFD 101]

## Summary

A conversation label holds a set of values rather than one: `crate=jp_config`
and `crate=jp_llm` coexist under the same key.
`jp c label` splits `set` off from `add` as a distinct verb, `rm` accepts
`key=value` as well as `key`, and filters match on set membership.
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

`set` is already accepted today as an alias for `add`, and it keeps its current
meaning: make this key hold exactly this.
Only `add` changes behaviour, from replacing to accumulating.

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

#### Argument grammar

Operands are resolved as a whole before anything is applied, so a verb naming
one key several times acts on the union rather than on each operand in turn:

1. Parse every operand; expand each `:name` alias to the key and ordered values
   its rule produces.
2. Group values by key, in the order the operands were given, discarding a
   repeat of a value already seen for that key.
3. Apply once per key: `add` inserts the grouped values, `set` replaces the
   key's set with them, `rm` removes them.

Without step 2, `jp c label set crate=jp_config crate=jp_llm` would replace the
set twice and leave `{jp_llm}`.
Both verbs accept aliases, and an alias contributes all of its values to its
key's group.

### Filtering

`key=value` matches when the key's set contains that value.
`key` matches when the key is present.

```sh
jp c ls --label=crate=jp_llm     # touches jp_llm, whatever else it touches
jp c ls --label=crate            # has any crate label
```

Repeated flags are ANDed, as they are under [RFD 101], so naming one key twice
requires the set to contain both values:

```sh
jp c ls --label=crate=jp_config --label=crate=jp_llm  # touches both
```

A set-expression syntax — exact-set matching, any-of, negation — is out of
scope here and belongs to the conversation query DSL.
"All of" needs no syntax of its own: it falls out of the conjunction rule above.

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
101], and no code path consults them while a query runs — so on the successful
path, labelling at creation and labelling immediately after are the same thing:

```sh
jp q --new "..." && jp c label add crate=jp_config crate=jp_llm
```

`jp query --new` always activates the conversation it creates, so the second
command needs no `--id`.
`jp conversation fork --activate` gives the same for a fork; a non-activating
fork needs the fork's ID, which [RFD 050] Phase 1 prints.

The two forms diverge when the turn fails.
`--label` applies before the provider is contacted, so a missing credential or a
dropped connection today leaves a labelled conversation; under `&&` it leaves an
unlabelled one.
Configured rules are unaffected either way — `apply_on.new` resolves during
creation — so what is lost is the ad-hoc labels of a failed turn, not the
automatic ones.
That is the case this RFD accepts in exchange for removing the flag; see
[Drawbacks](#drawbacks).

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

The accepted shapes, extending [RFD 101]'s table:

| TOML                                     | Resolved values           |
| ---------------------------------------- | ------------------------- |
| `crate = "jp_config"`                    | `{jp_config}`             |
| `crate = ["jp_config", "jp_llm"]`        | `{jp_config, jp_llm}`     |
| `crate = { value = "jp_config" }`        | `{jp_config}`             |
| `crate = { value = ["jp_config", ...] }` | the listed values         |
| `crate = { value = [] }`                 | no label                  |
| `crate = { value.cmd = "..." }`          | one value per stdout line |

The direct array is shorthand for `value`, mirroring how a bare string already
is.
A rule's values replace rather than extend across config layers: a workspace
rule naming two crates and a user rule naming one resolve to the user's one.
Per-entry accumulation would leave no way to narrow an inherited list, and the
map itself already carries `MergeableMap`'s strategies for merging *entries*.

Command output splits on newlines; empty lines are dropped and a command that
writes nothing produces no label.
Empty lines cannot be kept, because the empty string is a valid bare-label value
and a trailing newline would otherwise add one.
This replaces [RFD 101]'s whole-output trim.

**On fork, a matching rule replaces the key's set.** Inherited labels are the
starting state; each `apply_on.fork` rule then replaces the full set for its own
key, and a key with no matching rule is left alone.
That is [RFD 101]'s "re-resolved entries override the inherited values", carried
over unchanged — under a set-valued model "override" has to say whether it
means replace or extend, and replace is what preserves the existing precedence
order.
A source carrying `stage={draft, review}` under a rule producing `{approved,
ready}` forks to `{approved, ready}`, not to all four.

### Data model

```rust
// jp_conversation
pub struct Labels(BTreeMap<String, IndexSet<String>>);
```

The two halves are ordered differently on purpose.

`IndexSet` for values: order is preserved and is part of the contract.
Order carries meaning within a key — "mostly `jp_config`, also touches
`jp_llm`" — and a sorted view can be rendered from an ordered collection at any
time, while insertion order cannot be recovered from a sorted one.
The cost is that `metadata.json` is git-visible under [RFD 031], so the same
values added in different orders produce different bytes.
`conversation.attachments` already behaves that way.

`BTreeMap` for keys: no equivalent argument applies.
One key is not "more primary" than another, so making key order observable would
add diff noise to committed metadata and buy nothing.
Sorted keys also match what [RFD 101] already writes, so key ordering does not
change.

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

`Labels` also owns the on-disk contract, and deserializes through a validating
conversion rather than a derive.
A derived `Deserialize` would write the private field directly and could
construct exactly the states the API forbids — [RFD 031] supports editing
`metadata.json` by hand, so `{"crate": []}` is reachable.
Reading normalizes instead of failing, since a small manual slip should not cost
the whole conversation:

- A scalar becomes a one-element set.
- An array is deduplicated, first occurrence winning.
- An empty array drops the key, matching `set(key, empty)`.
- A non-string value is a metadata load error.

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

`jp c show` and `jp c label ls` render one row per key, with the values as a
list beneath it — the same numbered multi-line cell the details view already
uses for attachments and compactions.
Comma-separating them would be ambiguous: [RFD 101] places no restriction on
values, so `branch=feat,exp` is one legitimate value and `feat,exp, main` could
read as two values or three.

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

A mutation reports per key what the key held and what it holds now, so `set`'s
recoverability is a property of the output rather than of the reader's memory:

```json
{
  "action": "set",
  "conversation": { "id": "jp-c17866928997", "title": "Tool Chaining" },
  "changes": [
    { "key": "crate", "before": ["jp_config", "jp_llm"], "after": ["jp_cli"] }
  ]
}
```

[RFD 101]'s flat `labels` array cannot carry both halves, so it is replaced.
`add` and `rm` use the same shape; for them one side is a subset of the other.
The text form names the same before-and-after, so a `set` that displaced more
than the user expected is undoable from either.

## Drawbacks

- **A shipped surface is removed.** `--label` on `query` and `fork`, and
  `--reset-labels`, are described in [RFD 101] and implemented.
  Removing them costs a keystroke in the common interactive case and a second
  command in scripts.

- **Labelling stops being atomic with the turn.** `jp q --new --label=x` today
  writes the label before the provider is contacted, so it survives a failed
  turn; `jp q --new && jp c label add x` does not run the second command at all.
  A failed turn therefore leaves a conversation carrying its configured labels
  but none of the ad-hoc ones.
  Accepted because the labels that matter for categorisation come from rules,
  and because a flag that composes atomically has to answer the
  accumulate-or-replace question this RFD exists to avoid.

- **Order-dependent bytes in committed metadata.** Preserving value order means
  the same logical label set can produce different `metadata.json` diffs
  depending on the order values were added.
  Keys stay sorted, so this is confined to the values within one key.

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

### `IndexMap` for keys

Rejected: it would make key insertion order observable in `metadata.json`, in
`jp c show`, and in `jp c label ls`, for no gain.
The primacy argument that justifies ordered *values* has no counterpart across
keys, and [RFD 101] already writes keys sorted.

### Keep `--label` and pick a semantic

Either accumulate or replace, documented.
Rejected: the choice is permanent once released, the flag saves one `&&`, and
its interaction with configured rules is surprising under both readings.
It can be reintroduced later if the two-command form proves annoying in practice
— adding a flag is cheap, removing one is not.

## Non-Goals

- **Set-expression filter syntax.** Exact-set matching, any-of, and negation
  belong to the conversation query DSL.
  "All of" is not among them: repeated flags already AND, so it falls out of the
  filter semantics this RFD defines.
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

Mergeable independently, but only if the CLI keeps replacing: existing `Set`
directives call `set(key, one_element_set(value))`, not `insert`.
`insert` accumulates, so wiring it up in this phase would change `jp c label
add` from replacing to accumulating before the verb contract that explains it
lands in Phase 2.

### Phase 2: CLI and filters

`set` as a verb, `rm key=value`, contains-semantics for filters, display and
JSON changes.
Switches `add` from `set` to `insert`, now that a verb exists for replacing.

Removes `--label` from `jp query` and `jp conversation fork`, and
`--reset-labels` from `jp conversation fork`, along with the `LabelDirectives`
clap wrapper and both of its switches — `ALIASES`, which separates `jp query`
(where `--label=:name` resolves) from `jp conversation fork` (where it is
rejected), and `RESET`, which registers `--reset-labels`.
Alias support on `jp c label` is unaffected: it goes through
`LabelDirective::parse_set` and `expand_aliases`, not through the wrapper.

Depends on Phase 1.
Lands after [RFD 050] Phase 1, which prints the forked conversation's ID: until
then, removing `fork --label` leaves a fork that was deliberately not activated
with no way to name it.

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
