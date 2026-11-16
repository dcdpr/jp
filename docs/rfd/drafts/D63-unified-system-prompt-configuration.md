# RFD D63: Unified System Prompt Configuration

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-07
- **Summary**: Unifies `assistant.system_prompt` and
  `assistant.system_prompt_sections` into a single `assistant.system_prompts`
  list that accepts strings, tables, or entry arrays.

## Summary

This RFD unifies `assistant.system_prompt` and
`assistant.system_prompt_sections` into a single `assistant.system_prompts` key
that holds an ordered list of prompt entries.
The key accepts a bare string, a merge-metadata table, or a list of entries, and
retains `system_prompt` as an alias for backward compatibility.

## Motivation

JP has two config keys for the system prompt: `assistant.system_prompt` (a
mergeable string) and `assistant.system_prompt_sections` (a mergeable list).
Both concatenate into the same rendered system prompt, forcing users to choose
between two keys for the same concept.

String merging also prevents incremental diffs.
Appending to a merged string rewrites the entire scalar value, so downstream
consumers cannot tell what changed.
In a list of entries, each appended entry is a distinct element that can be
diffed individually.

This matters when switching personas mid-conversation.
Twelve persona files and `knowledge/software-laws.toml` set
`[assistant.system_prompt]` with `strategy = "append"`, layering on top of the
`strategy = "replace"` base in `personas/default.toml`.
Personas are selected with `--cfg=personas/<name>`, and four justfile recipes
apply a persona to an already running conversation:

```sh
jp query --id "$existing" --cfg=personas/pr-reviewer …
jp query --id "$existing" --cfg=personas/rfd-implementor …
```

Passing `--cfg` to an existing conversation records a `ConfigDelta` in the
conversation stream.
Because `assistant.system_prompt` is a merged string, switching personas
mid-conversation replaces the entire prompt string in the delta instead of
recording only the appended prompt entry.

Unifying on a list also removes the need for string-merge options.
Fields like `strategy`, `separator`, `dedup`, and `discard_when_merged` exist
only to make a scalar string behave like a list.

## Design

### Input Shapes

`assistant.system_prompts` accepts three TOML shapes, all of which deserialize
into a list of prompt entries:

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

Each entry in a list can be either a string or a table.
A string entry desugars to a table with `content` set and all other fields
defaulted, so the two forms are interchangeable:

```toml
assistant.system_prompts = [
    "Be concise.",
    { tag = "voice", content = "Write plainly.", position = -100 },
]
```

### `system_prompt` Alias

`assistant.system_prompt` remains an alias for `assistant.system_prompts` and
accepts all three input shapes.
The alias is supported in both file loading (`#[serde(alias = ...)]`) and
`--cfg` assignment (via a match arm in `PartialAssistantConfig::assign`).
Only the canonical `system_prompts` name appears in the generated JSON schema
and `jp init` output.

### Naming

| Before                                           | After                           |
| ------------------------------------------------ | ------------------------------- |
| `assistant.system_prompt_sections`               | `assistant.system_prompts`      |
| `SectionConfig`                                  | `SystemPromptConfig`            |
| `SectionConfigOrString`                          | `SystemPromptOrString`          |
| `assistant/sections.rs`                          | `assistant/prompts.rs`          |
| `ThreadBuilder::with_sections`, `build_sections` | `with_prompts`, `build_prompts` |

Once `assistant.system_prompt_sections` is renamed to
`assistant.system_prompts`, the term "section" no longer matches the config key.
All related types, modules, and builder methods are renamed to use "prompt"
instead, freeing "section" for future use elsewhere.

### Rendering

Prompt rendering is unchanged.
An entry with no `tag` and no `title` renders as its trimmed `content`, so a
converted string produces byte-identical output.
Entries remain ordered by `position`.

## Migration

Migration runs in three ordered steps so that stored configurations are
converted while `system_prompt` is still a known schema field:

1. **Convert the thirteen config files.** Rewrite each
   `[assistant.system_prompt]` block as an entry in `system_prompts`.
   Blocks that used `strategy = "append"` become additional list elements, using
   `position` to preserve the ordering produced by `separator = "paragraph"` and
   declaration order.
2. **Migrate stored conversations.** Run a one-off pass over `base_config.json`
   and `ConfigDelta` events to rewrite any recorded `assistant.system_prompt`
   into a `system_prompts` entry.
   Because `system_prompt` is still a schema field during this step,
   `compat::deserialize_partial_config` can read it without
   `strip_unknown_fields` dropping it.
3. **Remove the struct field.** Once all config files and stored conversations
   have been migrated to `system_prompts`, remove the separate `system_prompt`
   field from the config structs.

Reversing steps 2 and 3 would lose data: removing the field first would cause
`strip_unknown_fields` to drop every stored `system_prompt` during
deserialization, and old conversations would replay without their system prompt.

## Drawbacks

**Three input shapes on one key.** Accepting strings, tables, and lists on one
key widens the parsing surface and relies on implicit disambiguation.
`MergeableVec` dispatches on input shape via `serde_untagged`, which separates
strings, lists, and tables cleanly.
However, a single table must be tried as merge metadata (`MergedVec`) first and
as a single entry (`SystemPromptConfig`) second.
That disambiguation works only because `MergedVec` and `SystemPromptConfig` have
disjoint field names under `deny_unknown_fields`; adding a `strategy` field to
`SystemPromptConfig` later would silently misparse entry tables as merge
metadata.

**Requests change shape.** The twelve personas that append strings into a single
`assistant.system_prompt` value produce one system prompt block, whereas list
entries produce one block per entry.
The text and order are unchanged, but the request contains multiple blocks
(which Anthropic concatenates and other providers receive as separate system
messages).
Splitting entries into separate blocks is what makes them individually diffable,
but it means provider requests are not byte-identical across the migration.

**The alias keeps two spellings alive.** Supporting both `system_prompts` and
`system_prompt` leaves two names for one concept, which partially undercuts the
simplification.
However, removing the singular name would break every configuration that uses
`assistant.system_prompt`, including thirteen files in this repository.

## Alternatives

**Keep both keys.** Requires no migration, but leaves mid-conversation persona
switches undiffable: every persona applied to a running conversation rewrites
the full system prompt string and causes a full cache miss.

**Remove `system_prompt` with no alias.** Avoids maintaining two spellings, but
breaks every configuration that uses `assistant.system_prompt` without providing
any benefit over an alias.

**Fold `system_prompt` into an entry at finalize.** Keeps `system_prompt` in the
schema so `strip_unknown_fields` never drops stored values.
Rejected because merging happens on partial configs before finalize: the fold
step would receive a single already-merged string and produce a single entry,
leaving mid-conversation persona changes undiffable.

## Non-Goals

- **Directive behavior.** [RFD 105] defines how a changed prompt reaches the
  provider mid-conversation.
  This RFD only makes system prompt entries diffable.
- **`assistant.instructions`.** Instructions already compose as a separate list
  with their own rendering and need no change.
- **Reusing "section" elsewhere.** Retiring `SectionConfig` frees the term
  "section", but deciding where to use it next is out of scope.

## Risks and Open Questions

**`MergeableVec` has no `AssignKeyValue`.** As documented at
`types/vec.rs:32-37`, a generic `MergeableVec<T>` cannot tell whether `--cfg
key=value` should parse as `MergedVec` or as `T`.
Because `--cfg assistant.system_prompt=foo` must continue to work,
`system_prompts` needs a concrete rule matching the file shapes: a bare scalar
parses as an entry, and a table with merge-metadata field names parses as
`MergedVec`.

**Store migration correctness.** Step 2 rewrites durable conversation data in
`base_config.json` and `ConfigDelta` events.
The migration must be idempotent, leave conversations without a `system_prompt`
untouched, and be verifiable by replaying a migrated conversation and diffing
the resolved config against the pre-migration value.

**Entry-shape disambiguation.** Because a single table is tried as `MergedVec`
before `SystemPromptConfig` (see the Drawbacks section), a test should pin both
readings of a lone table so that adding a conflicting field to
`SystemPromptConfig` fails the test suite instead of silently reinterpreting
configurations.

## Implementation Plan

1. **Phase 1: Type and key rename.** Rename `SectionConfig` to
   `SystemPromptConfig`, `sections.rs` to `prompts.rs`, and
   `system_prompt_sections` to `system_prompts` with a serde alias for the old
   key.
   This phase has no behavior change and is independently mergeable.
2. **Phase 2: Polymorphic input.** Wire up the existing but unused
   `SectionConfigOrString` as the element type, add a `.string(...)` arm to
   `MergeableVec`'s `UntaggedEnumVisitor`, add scalar-or-list handling to
   `MergedVec::value`, and implement `AssignKeyValue` for the field.
   Depends on Phase 1.
3. **Phase 3: Convert config files.** Convert the thirteen persona and knowledge
   files to `assistant.system_prompts`, verifying the change by diffing rendered
   system prompts before and after.
   Depends on Phase 2.
4. **Phase 4: Store migration.** Run the one-off migration pass over
   `base_config.json` and `ConfigDelta` events.
   Depends on Phase 2, and must land before Phase 5.
5. **Phase 5: Remove `system_prompt` as a field.** Delete the `system_prompt`
   struct field while keeping `system_prompt` as an alias for `system_prompts`.
   Depends on Phases 3 and 4.

## References

- [RFD 105]: Mid-conversation operator directives, which motivate making persona
  switches diffable.
- [RFD 054]: Config deltas positioned in the conversation stream.
- [RFD 035]: Multi-root config load path resolution, which resolves
  `--cfg=personas/x`.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 054]: ../054-split-conversation-config-and-events.md
[RFD 105]: ../105-mid-conversation-operator-directives.md
