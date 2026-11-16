# RFD D64: Config Values Sourced from Files

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-20
- **Requires**: [RFD 079]
- **Summary**: Allows configuration fields to load their values from external
  files and JSON Pointers at config load time.

## Summary

This RFD allows opted-in configuration fields to load their values from another
file (and an optional JSON Pointer within that file) using a `{ file, pointer }`
table.
File references resolve relative to the declaring configuration file at load
time, before merging, without executing external commands.

## Motivation

Every config source in JP reads static configuration written directly as config:
the four file locations, `extends` targets, `JP_CFG_*` environment variables,
`--cfg` arguments ([RFD 079]), and the interactive editor ([RFD 080]).
None of these sources can derive a config value from project data that already
lives in the repository.

The in-repo ticket board ([RFD 100]) illustrates the gap.
It defines a closed label vocabulary in `docs/ticket/.labels.json` that declares
the valid label keys and their allowed values.
The `ticket_*` tools need that same list as a JSON Schema `enum` so the
assistant selects from valid labels rather than inventing them.
The vocabulary cannot move into the tool declarations because `jp ticket` and
the docs build both read `.labels.json` without loading JP's tool config, and
the file must travel with the tickets when `--dir` moves them.
As a result, the label list exists twice, and the two copies drift.

Existing workarounds all have drawbacks.
A test that compares the two files requires updating both whenever a label
changes, or CI fails.
A generator recipe still requires contributors to remember to run it after
editing `.labels.json`.
Dropping the `enum` from the tool schema avoids the duplication, but every
invalid guess by the model costs a corrective round trip, and on `ticket_create`
that round trip triggers an approval prompt for a call that will fail
validation.

This problem applies to any config field that mirrors project data.
Without a general mechanism in the config loader, each case requires its own
synchronization recipe or drift test.

## Design

### Reference Syntax

Any field that opts in accepts either a literal value or a reference table:

```toml
# Literal, exactly as today.
enum = ["client=cli", "client=web"]

# Reference.
enum = { file = "../../../docs/ticket/.labels.json", pointer = "/client/values" }
```

The reference table supports three fields:

| Field     | Required | Meaning                                                        |
| --------- | -------- | -------------------------------------------------------------- |
| `file`    | yes      | Path to the target file, relative to the declaring config file |
| `pointer` | no       | [RFC 6901] JSON Pointer into the document; defaults to root |
| `select`  | no       | `value` (default), or `keys` to take an object's keys          |

`file` is parsed with the config loader's existing format detection, so a
reference can target a TOML, JSON, or YAML file regardless of the declaring
file's format.

`select = "keys"` extracts the keys of a map as a list of strings.
Data files often map each item name to a description or metadata object; `select
= "keys"` lets config fields reference those names directly without maintaining
a separate array in the data file.

### Resolution

References resolve per file during config loading, in the same pass that
resolves `extends`:

1. Read the declaring config file into a raw document.
2. Locate each reference object in the document.
3. Resolve `file` relative to the declaring file's directory, parse the target
   file, apply `pointer` and `select`, and substitute the resulting value in
   place.
4. Pass the substituted document to the partial config parser.

Resolving references before parsing keeps the partial config types unchanged: by
the time `PartialAppConfig` is constructed, every value is a literal.
Resolving before merging scopes each reference to the file that declares it, so
a higher-precedence config layer overrides a file-sourced value the same way it
overrides a literal.

Referenced files are treated as plain data rather than config files: they are
read once at the requested pointer and do not participate in `extends`.

### Failure Behavior

Like `extends` on a missing non-glob target, a missing `file` logs a warning and
continues:

| Condition                         | Behavior                       |
| --------------------------------- | ------------------------------ |
| `file` missing                    | Warn and leave the field unset |
| `file` unparseable                | Error                          |
| `pointer` target missing          | Error                          |
| `select = "keys"` on a non-object | Error                          |
| Resolved value has the wrong type | Error from the existing parser |

Missing files are treated leniently because a reference may point to an optional
file.
Every other failure indicates a broken reference or invalid target data, where
failing at load time is easier to diagnose than silently leaving the field
unset.

### Opting Fields In

File sourcing does not change the `schematic` derive macro and does not apply to
every config field automatically.
A field opts in by wrapping its value type in `Sourced<T>`, following the
pattern `extends` uses with `ExtendingRelativePath`:

```rust
pub enum Sourced<T> {
    Literal(T),
    Reference(FileReference),
}
```

Scoping support to `Sourced<T>` leaves the merge, delta, and default-filling
machinery untouched and makes a field's support for file references explicit in
its Rust type.

### Provenance

Each resolved reference records its source file and pointer so [RFD 060] can
report `enum ← docs/ticket/.labels.json#/client/values` instead of an
unattributed value.

## Drawbacks

**Pressure to grow into a query language.** Once `pointer` and `select` exist,
callers will eventually ask for `filter` or `map`.
This RFD draws a hard boundary: a reference addresses a single location in a
single document and reads it without transformation.
Data transformations belong in the referenced file or the tool that writes it.
If a use case genuinely requires load-time transformation, that argues for the
command-execution design rejected below rather than expanding `pointer` and
`select`.

**Two forms for opted-in fields.** Every opted-in field accepts both a literal
value and a reference table, which complicates documentation, examples, and
parser error messages for a feature most fields never use.

**Config files are less self-contained.** Reading a config file no longer shows
all of its values directly.
Provenance in `--explain` helps when inspecting resolved config through the CLI,
but does not help someone reading the raw file.

**Only one immediate consumer.** Ticket labels are the only concrete use case so
far; the other candidates in the implementation plan are plausible but
unrequested.
If none of them materialize, this adds general loader machinery for a single
caller.

## Alternatives

**Command output as a config source.** A command source such as `extends = [{
command = "..." }]` that reads a subprocess's stdout is strictly more general:
it can fetch secrets from a keychain, compute per-machine paths, or produce
values that do not exist in a file.
Rejected because its security and performance costs are disproportionate to this
problem.
Config loads on every CLI invocation, so command sources would execute
repository-supplied commands even on `jp --help` in a freshly cloned checkout.
While `providers.mcp.*.command` and tool `command` settings already run
repository-supplied code, they run only when a tool is invoked and are governed
by [RFD 077].
A load-time equivalent would require its own trust model, output caching and
cache invalidation to avoid spawning processes on every invocation, and a way
for `--explain` to report output that changes between runs.
If dynamic command sources are needed later, a `command` variant can slot into
the same resolution pass under its own RFD.

**A drift test.** Duplicating the value in both files and asserting equality in
CI requires no loader changes and makes divergence visible before merge.
Rejected because it penalizes routine edits: changing project data fails an
unrelated test until the contributor also updates the second file.

**A generator recipe.** A `just ticket-labels-sync` recipe that rewrites the
derived file avoids manual edits, but still requires contributors to remember to
run it after changing the source data.
It is used as an interim workaround for ticket labels because it can be deleted
once this RFD lands.

**Format the data file as a config partial.** Shaping `.labels.json` as an
`AppConfig` fragment and loading it via `extends` avoids new syntax.
Rejected because the label vocabulary is ticket-board data: placing it in JP's
config namespace would force `jp ticket` and the docs build to parse `AppConfig`
partials, where per-label descriptions have no natural home.

**Self-describing tools ([RFD D06]).** Allowing a local tool to return its own
schema at call time and compute the `enum` itself would solve the ticket labels
case, but it does not help non-tool config fields and requires a much larger
change to the tool protocol.

## Non-Goals

- **Executing external commands.** References only read static files; see
  Alternatives.
- **Writing back to referenced files.** References are read-only; lossless
  config editing ([RFD D02]) does not follow a reference to edit its target.
- **Watching referenced files for changes.** References resolve once at load
  time.
  Detecting changed data files in a long-running host is covered by [RFD D36].
- **Network references.** `file` accepts local filesystem paths only.
- **Environment or shell expansion inside `file`.** Paths are resolved literally
  relative to the declaring file.

## Risks and Open Questions

**Is `select = "keys"` the right primitive, or an overly narrow special case?**
`select = "keys"` exists for maps keyed by item name, but an array of objects
with a `name` field is just as common and `keys` does not help there.
Supporting object arrays would require either reshaping the data file to expose
a flat array or adding a third `select` mode, which crosses into the query
language this RFD avoids.

**How does a reference interact with `--cfg` deltas and conversation config?**
References resolve at load time, so a recorded `ConfigDelta` stores the resolved
literal value rather than the reference.
Replaying a conversation therefore uses the value from when the turn was
recorded, even if the referenced file has since changed.
Keeping recorded conversations stable is desirable, but a persisted delta can
disagree with the current file on disk, and `--explain` should surface that
difference.

**Fragile relative paths.** Referencing `../../../docs/ticket/.labels.json` from
inside `.jp/mcp/tools/ticket/` breaks if either directory moves.
Workspace-root anchoring (`//docs/ticket/.labels.json`) would shorten and
stabilize these paths, but is deferred so it can align with the loader path
semantics in [RFD D35].

**Does warn-on-missing mask typos?** A misspelled `file` path logs a warning and
leaves the field unset, making it indistinguishable from an intentionally
optional file.
Returning an error on missing files would catch typos immediately, at the cost
of diverging from `extends`.

## Implementation Plan

**Phase 1: Reference types and resolver.** Add `FileReference` and `Sourced<T>`
in `jp_config::types`, and implement reference resolution in `jp_config::util`
alongside the `extends` walk.
No config field uses `Sourced<T>` yet; this phase is tested directly and can
merge on its own.

**Phase 2: Opt in tool parameter enums.** Wrap tool parameter `enum` fields
(`conversation.tools.*.parameters.*.items.enum`) in `Sourced<T>`, then delete
the label mirror file and the `just ticket-labels-sync` recipe added for [RFD
100].
Depends on Phase 1.

The ticket label vocabulary in `docs/ticket/.labels.json` is nested by key (each
with `values` and `retired` lists), whereas the tool schema expects `key=value`
strings rather than either list verbatim.
Because `pointer` and `select` cannot construct `key=value` strings from a
nested map, Phase 2 must either flatten the label file shape or concede that the
motivating consumer requires a transformation this design refuses.
Resolve that question before starting Phase 2, as it directly tests whether the
`pointer`-only boundary holds.

**Phase 3: Provenance.** Record the source path for each resolved reference and
surface it in `--explain`.
Depends on Phase 1 and [RFD 060], and can merge independently of Phase 2.

Candidate follow-up consumers (not committed to here) include model alias lists,
`assistant.instructions` shared across workspaces, and plugin registries.

## References

- [RFD 079]: Config Sources and Load Order, the load pipeline this RFD extends.
- [RFD 080]: Editor as a Config Source, another config source addition.
- [RFD 100]: In-Repo Ticket Tracking, the motivating use case.
- [RFD 060]: Config Explain, which consumes the provenance recorded in Phase 3.
- [RFD 077]: Plugin Configuration and Trust Policy, the trust model that a
  command-execution source would need to extend.
- [RFC 6901]: JSON Pointer specification.
- `crates/jp_config/src/types/extending_path.rs`: precedent for opting a field
  in through its Rust type.
- `crates/jp_config/src/util.rs`: `extends` resolution implementation.

[RFC 6901]: https://datatracker.ietf.org/doc/html/rfc6901
[RFD 060]: ../060-config-explain.md
[RFD 077]: ../077-plugin-configuration-and-trust-policy.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD 080]: ../080-editor-as-a-config-source.md
[RFD 100]: ../100-in-repo-ticket-tracking.md
[RFD D02]: D02-lossless-config-editing.md
[RFD D06]: D06-self-describing-local-tools.md
[RFD D35]: D35-extends-overrides-and-loader-namespace.md
[RFD D36]: D36-live-workspace-view-for-long-running-plugin-hosts.md
