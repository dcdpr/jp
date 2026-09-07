# RFD D59: Unified System Prompt Configuration

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-07

## Summary

`assistant.system_prompt` and `assistant.system_prompt_sections` collapse into
one key, `assistant.system_prompts`, holding an ordered list of prompt entries.
The key accepts a bare string, a merge-metadata table, or a list of entries, so
every shape in use today keeps working, and `system_prompt` survives as an
alias.

## Motivation

JP has two config keys for one concept. `assistant.system_prompt` is a mergeable
string; `assistant.system_prompt_sections` is a mergeable list. Both end up
concatenated into the same system prompt, and a user has to know which to reach
for.

The split costs more than confusion. A merged string has no addressable
identity: appending to it rewrites the whole value, so nothing downstream can
tell what changed. A list of entries is diffable by construction — an appended
element is a new element.

That matters because of how personas work. Twelve persona files and
`knowledge/software-laws.toml` set `[assistant.system_prompt]`, composing with
`strategy = "append"` on top of the `strategy = "replace"` base in
`personas/default.toml`. Personas are selected with `--cfg=personas/<name>`, and
four justfile recipes apply one to an *already running* conversation:

```sh
jp query --id "$existing" --cfg=personas/pr-reviewer …
jp query --id "$existing" --cfg=personas/rfd-implementor …
```

A `--cfg` on an existing conversation is recorded as a `ConfigDelta` in the
stream. So switching persona mid-conversation is the most common operator-level
change JP makes, and it arrives in the one shape that cannot be expressed
incrementally.

The string-merge machinery is the other half of the argument. `strategy`,
`separator`, `dedup`, and `discard_when_merged` exist to make a scalar behave
like a list. A list needs none of them.

## Design

### One key, three shapes

All three forms are valid on the same key, and all three produce a list:

```toml
# A bare string: one untagged entry.
assistant.system_prompts = "You are a helpful assistant."

# Merge metadata, with a string value.
[assistant.system_prompts]
strategy = "append"
separator = "paragraph"
value = "You are a principal software architect."

# A list of entries, each with the full entry shape.
[[assistant.system_prompts]]
position = -100
tag = "voice"
content = "Write plainly."
```

An entry is either a string or a table. A string entry becomes a table with
`content` set and everything else defaulted, so the two are interchangeable:

```toml
assistant.system_prompts = [
    "Be concise.",
    { tag = "voice", content = "Write plainly.", position = -100 },
]
```

### `system_prompt` is an alias

`assistant.system_prompt` continues to parse, addressing the same key. Every
example above works spelled either way. The alias covers both entry points:
`#[serde(alias = ...)]` for file loading, and a match arm in
`PartialAssistantConfig::assign` for `--cfg`. Only the canonical name appears in
the generated JSON schema and in `jp init` output.

### Naming

| Today | After |
| ----- | ----- |
| `assistant.system_prompt_sections` | `assistant.system_prompts` |
| `SectionConfig` | `SystemPromptConfig` |
| `SectionConfigOrString` | `SystemPromptOrString` |
| `assistant/sections.rs` | `assistant/prompts.rs` |
| `ThreadBuilder::with_sections`, `build_sections` | `with_prompts`, `build_prompts` |

"Section" stops describing anything once the key is not named after it, so it is
retired from this area entirely and freed for later use elsewhere.

### Rendering is unchanged

An entry with no `tag` and no `title` renders as its trimmed content, so a
converted string produces byte-identical output. Ordering still comes from
`position`.

## Migration

Three steps, in order. The ordering is the point: converting the data while the
old field still exists means nothing is ever stripped.

**1. Convert the thirteen config files.** Each `[assistant.system_prompt]` block
becomes an entry in `system_prompts`. `strategy = "append"` becomes another
element; `position` reproduces the ordering that `separator = "paragraph"` and
declaration order give today.

**2. Migrate stored conversations.** A one-off pass rewrites any recorded
`assistant.system_prompt` in `base_config.json` and in `ConfigDelta` events into
an entry. This runs while `system_prompt` is still a schema field, so
`compat::deserialize_partial_config` can still read it.

**3. Remove the field.** By this point nothing carries it, so there is nothing
for `strip_unknown_fields` to drop.

Reversing steps 2 and 3 loses data: removing the field first makes every stored
`system_prompt` unreadable, and old conversations replay without it.

## Drawbacks

**Three input shapes on one key.** More parsing surface, and the
disambiguation is implicit. `MergeableVec` dispatches on input shape via
`serde_untagged`, which handles string-versus-list-versus-table cleanly. But a
lone table has to be tried as merge metadata first and as a single entry second,
and that only works because `MergedVec` and `SystemPromptConfig` have disjoint
field names under `deny_unknown_fields`. Adding a `strategy` field to
`SystemPromptConfig` later would break it silently.

**Requests change shape.** Twelve personas that append string-onto-string
produce one system block today; as list entries they produce one block each.
Same text, same order, more blocks — concatenated by Anthropic, one system
message each elsewhere. That is the change that makes them individually
diffable, but it means requests are not byte-identical across the migration.

**The alias keeps two spellings alive.** One concept, two names, which
partially undercuts the simplification. The alternative is breaking every
config that uses the singular, including thirteen in this repo.

## Alternatives

**Keep both keys.** Zero work, and the persona case stays permanently
undiffable — every persona applied to a running conversation costs a full cache
miss.

**Remove `system_prompt` with no alias.** Simpler surface, but breaks every
config using the singular for no gain the alias does not already provide.

**Fold `system_prompt` into an entry at finalize.** Keeps the field and the
schema, so nothing is ever stripped. Rejected: merging happens on partials
*before* finalize, so the fold receives one already-merged string and produces
one entry. The scalar is still a scalar, and the persona case is still
undiffable. Cosmetic only.

## Non-Goals

- **Directive behaviour.** How a changed prompt reaches the provider
  mid-conversation is [RFD 105]'s subject. This RFD only makes the content
  diffable.
- **`assistant.instructions`.** A separate list with its own rendering; it
  already composes as a list and needs no change.
- **Reusing "section" elsewhere.** The term is vacated here; what claims it next
  is a later decision.

## Risks and Open Questions

**`MergeableVec` has no `AssignKeyValue`.** Deliberately so: the doc comment at
`types/vec.rs:32-37` records the reason — with a generic `T`, it is unclear
whether `--cfg key=value` should parse as `MergedVec` or as `T`. This RFD has to
answer that for `system_prompts`, since `--cfg assistant.system_prompt=foo` must
work. The answer follows the file shapes: a bare scalar is an entry, a table
with merge-metadata field names is `MergedVec`.

**Store migration correctness.** Step 2 rewrites durable conversation data. It
needs to be idempotent, to leave conversations without a `system_prompt`
untouched, and to be verifiable by replaying a migrated conversation and
diffing the resolved config against the pre-migration value.

**Entry-shape disambiguation.** See Drawbacks. Worth a test that pins both
readings of a lone table, so a future field addition to `SystemPromptConfig`
fails loudly rather than silently reinterpreting configs.

## Implementation Plan

**Phase 1 — Type and key rename.** `SectionConfig` to `SystemPromptConfig`,
`sections.rs` to `prompts.rs`, `system_prompt_sections` to `system_prompts` with
a serde alias for the old key. No behaviour change. Independently mergeable.

**Phase 2 — Polymorphic input.** Wire up the existing but unused
`SectionConfigOrString` as the element type, add a `.string(...)` arm to
`MergeableVec`'s `UntaggedEnumVisitor`, add scalar-or-list handling to
`MergedVec::value`, and implement `AssignKeyValue` for the field. Depends on
phase 1.

**Phase 3 — Convert config files.** The thirteen persona and knowledge files.
Verifiable by diffing rendered system prompts before and after. Depends on
phase 2.

**Phase 4 — Store migration.** The one-off pass over `base_config.json` and
`ConfigDelta` events. Depends on phase 2, and must land before phase 5.

**Phase 5 — Remove `system_prompt` as a field**, keeping it as an alias for
`system_prompts`. Depends on phases 3 and 4.

## References

- [RFD 105] — mid-conversation operator directives, the reason the persona case
  matters
- [RFD 054] — config deltas positioned in the conversation stream
- [RFD 035] — multi-root config load path resolution, how `--cfg=personas/x`
  resolves

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 054]: ../054-split-conversation-config-and-events.md
[RFD 105]: ../105-mid-conversation-operator-directives.md
