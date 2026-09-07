# RFD D38: Config Values Sourced from Files

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-20
- **Requires**: [RFD 079]

## Summary

A config field can take its value from another file instead of spelling it out:

```toml
[conversation.tools.ticket_create.parameters.labels.items]
enum = { file = "../../../docs/ticket/.labels.json", pointer = "/package/values" }
```

The reference resolves at load time, relative to the declaring file, before
merging.
Nothing is executed, and the result is the same on every invocation.

## Motivation

Every config source JP has today is a static byte stream someone typed: the four
file locations, the `extends` targets, `JP_CFG_*`, `--cfg` entries.
[RFD 079] documents them; [RFD 080] adds the editor as another.

That covers config a human writes.
It does not cover config that is a *function of project data the repository
already holds*.

The case that forced the question: the in-repo ticket board ([RFD 100]) has a
closed label vocabulary in `docs/ticket/.labels.json`, declaring which keys
exist and which values each accepts.
The `ticket_*` tools need that same list as a JSON Schema `enum`, so the
assistant is offered valid labels rather than inventing them.
The vocabulary cannot move into the tool declarations: `jp ticket` and the docs
build both read it, neither loads JP's tool config, and the file has to travel
with the tickets when `--dir` moves them.
So the list exists twice, and the copies drift.

The available answers are all bad in the same way.
A test comparing the two turns "add a label" into "add a label and update a
second file or CI goes red", which taxes an edit that should cost nothing.
A generator recipe moves the tax rather than removing it, and only helps the
people who remember to run it.
Dropping the `enum` costs a corrective round trip every time the model guesses,
and on `ticket_create` that round trip is an approval prompt for a call that
cannot land.

None of that is specific to labels.
Any config value that mirrors project data has it.

Doing nothing means each such case grows its own bespoke sync, and the generic
capability never arrives because no single case justifies it alone.

## Design

### What the user writes

Any field that opts in accepts either a literal or a reference object:

```toml
# Literal, exactly as today.
enum = ["client=cli", "client=web"]

# Reference.
enum = { file = "../../../docs/ticket/.labels.json", pointer = "/client/values" }
```

The reference has three fields:

| Field     | Required | Meaning                                                        |
| --------- | -------- | -------------------------------------------------------------- |
| `file`    | yes      | Path, relative to the file declaring it                        |
| `pointer` | no       | [RFC 6901] JSON Pointer into the document; defaults to root |
| `select`  | no       | `value` (default), or `keys` to take an object's keys          |

`file` is read with the same format detection the config loader already uses, so
a reference can point at TOML, JSON, or YAML regardless of the declaring file's
format.

`select = "keys"` exists because "the names in this map" is the common shape for
data files that pair a name with a description.
Without it, every such file needs a parallel array.

### Resolution

References resolve during load, per file, in the same pass that resolves
`extends`:

1. Read the declaring file into a raw document.
2. Walk it for reference objects.
3. For each, resolve `file` relative to the declaring file's directory, read it,
   apply `pointer`, apply `select`, substitute the result.
4. Hand the substituted document to the partial parser.

Resolving before parsing rather than after keeps the partial types unchanged: by
the time `PartialAppConfig` exists, every value is a literal.
Resolving before *merging* means a reference is a property of the file that
declares it, not of the merged result, so a higher layer overrides a
reference-derived value exactly as it overrides a literal one.

Referenced files do not participate in `extends` and are not themselves config:
they are data, read once, at one pointer.

### Failure behavior

Matching `extends`, which warns and continues on a missing non-glob target:

| Condition                         | Behavior                        |
| --------------------------------- | ------------------------------- |
| `file` missing                    | Warn, leave the field unset     |
| `file` unparseable                | Error                           |
| `pointer` names nothing           | Error                           |
| `select = "keys"` on a non-object | Error                           |
| Resolved value has the wrong type | Error, from the existing parser |

A missing file is the lenient case because a reference can legitimately point at
something optional.
Everything else means the reference is wrong, and a silently unset field would
be harder to diagnose than a failed load.

### Opting a field in

Not a change to the `schematic` derive, and not a capability every field gets.
A field opts in by declaring a type that accepts the reference form, the way
`extends` opts into `ExtendingRelativePath` today:

```rust
pub enum Sourced<T> {
    Literal(T),
    Reference(FileReference),
}
```

This keeps the blast radius to the fields that want it, keeps the merge, delta,
and fill machinery untouched, and makes "can this field be sourced from a file?"
answerable from its type.

### Provenance

Each resolved reference records the file it came from, so [RFD 060] can report
`enum ← docs/ticket/.labels.json#/client/values` rather than a bare value.

## Drawbacks

**A query language wants to grow here.** `pointer` and `select` are two knobs;
the third request will be `filter`, the fourth `map`.
The line this RFD draws: a reference addresses *one location* in *one document*
and takes it whole.
Anything that transforms content belongs in the file being referenced, or in
whatever writes it.
If a case genuinely needs transformation, that is evidence for the
command-execution design this RFD rejects, not for growing `pointer`.

**Two ways to write the same thing.** Every opted-in field now has a literal
form and a reference form, in docs, in examples, in error messages.
That is a real cost paid by every reader, for a feature most fields never use.

**Config stops being self-contained.** Reading a config file no longer tells you
its values.
Provenance in `--explain` mitigates this for anyone who thinks to look; nothing
mitigates it for someone reading the file directly.

**It is one consumer today.** Labels is the forcing case.
The other candidates named below are plausible, not requested.
If none of them materialize, this is machinery serving a single caller, which is
the midlayer mistake.

## Alternatives

**Command output as a config source.** `extends = [{ command = "..." }]`, with
the source being a process's stdout.
Strictly more general: it covers secrets from a keychain, per-machine paths,
anything a file cannot answer.
Rejected because the cost is not proportional to this problem.
Config loads on every invocation, so this executes repository-supplied commands
on `jp --help` in a freshly cloned checkout, before any user intent.
`providers.mcp.*.command` and tool `command` already run repo-supplied code, but
only when a tool is actually invoked, and [RFD 077] exists to govern exactly
that.
A load-time equivalent needs its own trust design, plus caching (because a
process spawn per source per invocation is an inner-loop cost), plus cache
invalidation, plus an answer for what `--explain` reports when the output
differs between runs.
If dynamism is ever wanted, it should be its own RFD carrying its own trust
argument.
Nothing here forecloses it: a `command` variant slots into the same resolution
step.

**A drift test.** Duplicate the value, compare in CI.
Zero new machinery, and the duplication becomes visible rather than silent.
Rejected because it taxes the edit: changing project data now fails an unrelated
test until a second file is updated.
That is the wrong incentive on data that should be cheap to change.

**A generator recipe.** `just ticket-labels-sync` rewrites the derived file.
Rejected as a permanent answer for the same reason, one step removed: it does
not fail, but it still requires remembering.
Accepted as the *interim* for labels, precisely because it is deletable when
this RFD lands.

**Make the data file a config partial.** Shape `.labels.json` as an `AppConfig`
fragment and `extends` it.
Rejected: the vocabulary is ticket-board data, and this would put it in JP's
config namespace where `jp ticket` and the docs build cannot reasonably read it,
and where per-label descriptions have no natural home.

**Self-describing tools ([RFD D06]).** A local tool could return its own schema
at call time, computing the `enum` itself.
This solves the labels case and nothing else, does not help any non-tool field,
and is a much larger change to the tool protocol.

## Non-Goals

- **Executing anything.** See Alternatives.
- **Writing back.** A reference is read-only.
  [RFD D02]'s lossless editing does not follow the reference to edit the target.
- **Watching for changes.** Resolution happens at load.
  A long-running host that wants to notice a changed data file is [RFD D36]'s
  problem.
- **Referencing across the network.** Local paths only.
- **Environment or shell expansion inside `file`.** A path is a path.

## Risks and Open Questions

**Is `select = "keys"` the right primitive, or a special case in disguise?** It
exists for one shape (a map of name to description).
An array of objects with a `name` field is just as common and `keys` does not
help there.
Either the answer is "reference an array and shape the data file accordingly",
or `select` needs a third value, and that is the slope this RFD claims not to
slide down.

**How does a reference interact with `--cfg` deltas and conversation config?** A
reference resolves at load; the resolved value is what lands in a recorded
`ConfigDelta`.
That means a conversation replays the value as it was, not as the file now
reads.
Probably correct (a recorded conversation should be stable), but it means a
delta can disagree with the file, and `--explain` should say so.

**Relative paths are long.** `../../../docs/ticket/.labels.json` from inside
`.jp/mcp/tools/ticket/` is fragile against moving either end.
Workspace-root anchoring (`//docs/ticket/.labels.json`) would help.
Deferred rather than decided, since [RFD D35] is already reworking loader path
semantics and the two should agree.

**Does the warn-on-missing case hide typos?** A misspelled `file` is
indistinguishable from an intentionally optional one.
The alternative is erroring on missing files, which diverges from `extends`.
Worth revisiting if it bites.

## Implementation Plan

**Phase 1: the reference type and resolver.** `FileReference` and `Sourced<T>`
in `jp_config::types`, plus resolution in `jp_config::util` alongside the
`extends` walk.
No field uses it yet; tested directly.
Mergeable alone.

**Phase 2: opt in the first field.** `enum` on tool parameters
(`conversation.tools.*.parameters.*.items.enum`).
Delete the label mirror file and the `just ticket-labels-sync` recipe added for
[RFD 100].
Depends on phase 1.

The ticket vocabulary is nested (key, then `values` and `retired`), and the
tokens the schema wants are `key=value` strings rather than either list
verbatim.
A pointer plus `select` cannot build them, so phase 2 either accepts a flatter
mirror shape or concedes that this consumer needs a transformation the design
refuses.
Resolve that before starting: it is the sharpest test of whether the
`pointer`-only line holds.

**Phase 3: provenance.** Record the source path per resolved reference and
surface it in `--explain`.
Depends on phase 1 and on [RFD 060]; mergeable independently of phase 2.

Candidate consumers after phase 2, none of them committed to here: model alias
lists, `assistant.instructions` shared across workspaces, plugin registries.

## References

- [RFD 079]: Config Sources and Load Order — the load pipeline this extends.
- [RFD 080]: Editor as a Config Source — the other in-flight source addition.
- [RFD 100]: In-Repo Ticket Tracking — the forcing case.
- [RFD 060]: Config Explain — consumer of the provenance in phase 3.
- [RFD 077]: Plugin Configuration and Trust Policy — the trust model a
  command-execution variant would have to extend.
- [RFC 6901]: JSON Pointer.
- `crates/jp_config/src/types/extending_path.rs` — the opt-in-by-type
  precedent.
- `crates/jp_config/src/util.rs` — where `extends` resolution lives.

[RFC 6901]: https://datatracker.ietf.org/doc/html/rfc6901
[RFD 060]: ../060-config-explain.md
[RFD 077]: ../077-plugin-configuration-and-trust-policy.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD 080]: ../080-editor-as-a-config-source.md
[RFD 100]: ../100-in-repo-ticket-tracking.md
[RFD D02]: D02-lossless-config-editing.md
[RFD D06]: D06-self-describing-local-tools.md
[RFD D35]: D35-loader-namespace-and-extends-overrides.md
[RFD D36]: D36-live-workspace-view-for-long-running-plugin-hosts.md
