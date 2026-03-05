# RFD 008: Knowledge Base

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23

## Summary

This RFD proposes a knowledge base (KB) system that organizes reusable,
project-specific knowledge into **topics** and **subjects**. Topics are
configured directories of text files. The assistant discovers available topics
through the system prompt and retrieves subjects on demand via a built-in
`learn` tool. Subjects can also be pre-loaded into the system prompt or injected
per-query via the `-k` CLI flag.

## Motivation

JP's assistant operates with whatever context it receives: the system prompt,
conversation history, attachments, and tool call results. For project-specific
knowledge — coding conventions, architecture decisions, team information, skill
guides — users currently have two options:

1. **Inline everything in the system prompt.** This works for small amounts of
   knowledge but doesn't scale. A system prompt stuffed with every convention
   and guide wastes context window on knowledge the assistant may never need for
   a given query.

2. **Attach files per query.** The attachment system (`-a`) loads files into
   each query, but attachments are designed for ephemeral, query-specific
   context (a file to review, command output to analyze). They lack structure
   for organizing persistent project knowledge, and there is no way for the
   assistant to discover or request knowledge it doesn't know about.

> !REVIEW:
>
> Attachments are also added to the system prompt, they are just a convenient
> way to attach "something" that can be fetched using a handler (e.g. `file://`
> or `https://`), and thus are mostly similar to "inline everything in the
> system prompt" approach.

Neither approach gives the assistant a structured way to discover what knowledge
is available and pull in only what it needs. The knowledge base fills this gap:
a lightweight, file-based system where the assistant sees a menu of available
topics and retrieves specific subjects on demand.

If we do nothing, users continue to either over-stuff the system prompt or
manually attach files — both of which degrade quality for non-trivial projects
with meaningful domain knowledge.

## Design

### User-Facing Behavior

A user configures topics in their workspace configuration. Each topic points to
a directory of text files (subjects):

```toml
[kb.topic.project]
title = "General Project Knowledge"
introduction = "foo bar baz..."
subjects = ".jp/kb/project"

[kb.topic.skills]
title = "Learnable Assistant Skills"
subjects = ".jp/kb/skills"
```

> !REVIEW:
>
> I wonder if we should move `kb` into the `assistant` section, to avoid
> overwhelming the root level of the config.

The assistant sees available topics in its system prompt and retrieves subjects
by calling the `learn` tool. Users can also pre-load subjects into the system
prompt or inject them per-query:

```sh
# Pre-load via CLI flag
jp query -k "project/maintainers/*" "Review this PR"

# Or via config
jp query --cfg kb.topic.project.learned+="maintainers/*" "Review this PR"
```

### Design Goals

| Goal | Description |
|------|-------------|
| **Structured knowledge** | Organize knowledge into topics with subjects |
| **On-demand retrieval** | Assistant fetches subjects via `learn` tool |
| **Hidden subjects** | `.`-prefixed paths excluded from listings |
| **Pre-loaded subjects** | Inject critical knowledge into system prompt |
| **Glob support** | Load multiple subjects with patterns |

### Configuration Schema

`kb` is a top-level field on `AppConfig`, alongside `assistant` and
`conversation`:

```toml
# All fields shown — only `subjects` is required.
[kb.topic.project]
enable = true
title = "General Project Knowledge"
introduction = "foo bar baz..."
description = "longer initial description talking about this"
subjects = ".jp/kb/project"
learned = []
disabled = []

# Minimal — just the title and subjects directory.
[kb.topic.skills]
title = "Learnable Assistant Skills"
subjects = ".jp/kb/skills"
```

#### Rust Types

```rust
// jp_config/src/kb.rs

/// Knowledge base configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct KnowledgeBaseConfig {
    /// Map of topic ID → topic configuration.
    #[setting(nested, flatten, merge = merge_nested_indexmap)]
    pub topics: IndexMap<String, TopicConfig>,
}

/// A single knowledge base topic.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TopicConfig {
    /// Whether this topic is active.
    #[setting(default = true)]
    pub enable: bool,

    /// Human-readable title. Replaces the topic ID in the system
    /// prompt and `learn` tool output when set.
    pub title: Option<String>,

    /// One-sentence summary for the `<knowledge>` system prompt
    /// section.
    pub introduction: Option<String>,

    /// Multi-paragraph description. Shown in `learn` output when
    /// no specific subjects are requested. Also included in the
    /// system prompt when one or more subjects are `learned`.
    pub description: Option<String>,

    /// Relative path from workspace root to the directory
    /// containing this topic's subject files.
    #[setting(required)]
    pub subjects: RelativePathBuf,

    /// Glob patterns for subjects to pre-load into the system
    /// prompt. Matched subjects are excluded from the `learn`
    /// tool to avoid duplication.
    #[setting(default = vec![])]
    pub learned: Vec<String>,

    /// Subject slugs to exclude entirely. Disabled subjects
    /// cannot be learned, even by exact reference. Overrides
    /// `learned`.
    #[setting(default = vec![])]
    pub disabled: Vec<String>,
}
```

#### Field Reference

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `enable` | No | `true` | Set `false` to deactivate the topic |
| `title` | No | — | Display name shown to the assistant |
| `introduction` | No | — | One-line summary in system prompt |
| `description` | No | — | Extended description in `learn` output |
| `subjects` | **Yes** | — | Path to subject directory |
| `learned` | No | `[]` | Glob patterns for pre-loaded subjects |
| `disabled` | No | `[]` | Excluded subject slugs |

### Subject Resolution

A **subject** is a text file inside a topic's `subjects` directory. The **slug**
is the file path relative to the topic directory, with the file extension
stripped.

#### Directory Layout Example

```
.jp/kb/project/
├── maintainers/
│   ├── jean.md          → slug: "maintainers/jean"
│   └── ryan.md          → slug: "maintainers/ryan"
├── code-quality.md      → slug: "code-quality"
└── .internal-notes.md   → slug: "internal-notes" (hidden)

.jp/kb/skills/
├── ast-grep.md          → slug: "ast-grep"
└── ast-grep/
    └── .rules.md        → slug: "ast-grep/rules" (hidden)
```

#### Hidden Subjects

A subject is **hidden** when any component of its path starts with `.`:

| Path | Hidden? | Reason |
|------|---------|--------|
| `.internal-notes.md` | Yes | Filename starts with `.` |
| `ast-grep/.rules.md` | Yes | Nested filename starts with `.` |
| `.hidden-dir/visible.md` | Yes | Parent directory starts with `.` |
| `maintainers/jean.md` | No | No `.`-prefixed components |

Hidden subjects:

- **Not listed** by `learn` (even with `*` or `**` globs)
- **Loadable** only by exact slug: `learn(topic, subjects: ["ast-grep/rules"])`
- **Discoverable** only via external hints — e.g., a non-hidden subject mentions
  "read `ast-grep/rules` for full rule documentation"

#### Disabled Subjects

Subjects matching entries in the `disabled` array are fully excluded:

- Not listed by `learn`
- Not loadable, even by exact slug
- Override `learned` — a subject in both `disabled` and `learned` is disabled

#### Resolution Algorithm

```
resolve_subjects(topic_config, glob_patterns):
    base = workspace_root / topic_config.subjects

    # 1. Walk directory tree
    all_files = walk(base, recursive=true)

    # 2. Compute slugs (relative path, strip extension)
    all_slugs = [strip_ext(relative(f, base)) for f in all_files]

    # 3. Filter disabled
    available = [s for s in all_slugs if s not in disabled]

    # 4. If no patterns: return non-hidden available slugs
    if glob_patterns is None:
        return [s for s in available if not is_hidden(s)]

    # 5. Apply glob patterns
    matched = []
    for pattern in glob_patterns:
        for slug in available:
            if glob_match(pattern, slug):
                # Globs skip hidden subjects
                if not is_hidden(slug):
                    matched.append(slug)
            elif slug == pattern:
                # Exact match loads hidden subjects too
                matched.append(slug)

    return deduplicate(matched)
```

### System Prompt Injection

#### Knowledge Section

When at least one topic has available (non-hidden, non-disabled) subjects, the
system prompt includes a `<knowledge>` section:

```xml
<knowledge>
The following knowledge topics are available to learn:

- project (**General Project Knowledge**): foo bar baz...
- skills (**Learnable Assistant Skills**)

Use the `learn` tool to consume this knowledge.

(note: some topics may contain hidden subjects that are not listed via `learn`
by default, but can be loaded manually if you are made aware of their names via
other means, such as by reading non-hidden subjects first. This prevents
exposing too much irrelevant knowledge upfront)
</knowledge>
```

**Generation rules:**

1. Only list enabled topics with at least one non-hidden, non-disabled subject
   that is NOT already `learned`.
2. Show the `title` in bold after the topic ID, when present.
3. Append the `introduction` after the title, when present.
4. Omit topics where ALL subjects are pre-loaded via `learned`.

#### Pre-loaded Knowledge

When a topic has `learned` patterns matching subjects, those subjects are
expanded inline in the system prompt:

```xml
<knowledge>
The following knowledge has been pre-loaded into your system prompt:

<topic "General Project Knowledge">

[[topic description here]]

<subject "maintainers/jean">
...file content...
</subject>

<subject "maintainers/ryan">
...file content...
</subject>
</topic>

The following knowledge topics are available to learn:

- skills (**Learnable Assistant Skills**)

Use the `learn` tool to consume this knowledge.

(note: some topics may contain hidden subjects that are not listed via `learn`
by default, but can be loaded manually if you are made aware of their names via
other means, such as by reading non-hidden subjects first. This prevents
exposing too much irrelevant knowledge upfront)
</knowledge>
```

Pre-loaded subjects do NOT appear in `learn` tool listings. They are shown in a
separate "already learned" section when `learn` is called without `subjects`, so
the assistant knows they exist.

#### Why Not Configurable Sections

The existing `SectionConfig` type (content, tag, position) is designed for
user-controlled system prompt sections. The `<knowledge>` section is **not
user-configurable** — its content is derived from KB config. It is built
programmatically and injected directly into the system prompt string during
thread construction.

Internally, the implementation MAY use `SectionConfig` as a container (with a
fixed tag of `"knowledge"` and a low position value), but this is an
implementation detail, not a user-facing feature.

#### Injection Point

The knowledge section is built during `Query::run`, after config is resolved but
before the LLM request is built:

```rust
// Pseudo-code
let kb_section = build_knowledge_section(
    &config.kb,
    &workspace_root,
)?;

if let Some(section) = kb_section {
    system_prompt_sections.push(section);
}
```

### The `learn` Tool

#### Registration

`learn` is registered as a built-in tool via `ToolSource::Builtin`. Its
definition is generated dynamically based on the KB configuration at query time.

```rust
// Pseudo-code for registration
if config.kb.has_learnable_topics() {
    let learn_definition = ToolDefinition {
        name: "learn".to_owned(),
        description: Some(
            "Learn about knowledge base topics and subjects."
                .to_owned(),
        ),
        parameters: build_learn_parameters(&config.kb),
    };

    tool_definitions.push(learn_definition);
}
```

The tool is only registered when at least one topic has subjects available to
learn (not all pre-loaded or disabled).

#### JSON Schema

The `topic` parameter is a free-form string. The description lists available
topics so the LLM knows what to pass. The schema itself is stable — it does not
change when the KB configuration changes.

```json
{
  "type": "object",
  "properties": {
    "topic": {
      "type": "string",
      "description": "The topic ID or title to learn about."
    },
    "subjects": {
      "type": [
        "string",
        "array",
        "null"
      ],
      "description": "Glob pattern(s) for subjects to load. Use * for current level, ** for recursive. Omit to list available subjects.",
      "items": {
        "type": "string"
      }
    }
  },
  "required": [
    "topic"
  ],
  "additionalProperties": false
}
```

The description is dynamically generated from the KB configuration (listing
available topic IDs and titles). The schema structure remains constant. This
avoids issues with providers caching stale tool schemas between turns.

#### Topic Resolution

When the assistant provides a `topic` value:

1. Exact match on topic ID → resolved
2. Case-insensitive match on title → resolved
3. No match → return error listing valid topics

#### Behavior: List Subjects (no `subjects` argument)

```
learn(topic: "skills")
```

```markdown
# Topic: Learnable Assistant Skills

[[optional topic description]]

## Available subjects:

- ast-grep

Use the `learn` tool with the `subjects` argument to learn specific subjects.
```

When some subjects are pre-loaded:

```markdown
# Topic: General Project Knowledge

[[optional topic description]]

## Available subjects:

- code-quality

Use the `learn` tool with the `subjects` argument to learn specific subjects.

## Already learned (in system prompt):

- maintainers/jean
- maintainers/ryan
```

#### Behavior: Load Subjects (with `subjects` argument)

Single subject:

```js
learn(topic: "skills", subjects: ["ast-grep"])
```

Returns the raw file content of `.jp/kb/skills/ast-grep.md` (with format
handling applied — see [File Format Handling](#file-format-handling)).

Multiple subjects (or glob):

```js
learn(topic: "skills", subjects: ["**"])
```

Returns all matching non-hidden subjects, each wrapped in tags:

```xml
<subject "ast-grep">
...content...
</subject>
```

#### Glob Behavior

Standard glob semantics. `*` matches at one directory level. `**` matches
recursively across directories.

| Pattern | Matches |
|---------|---------|
| `*` | Non-hidden subjects at the top level only |
| `**` | All non-hidden subjects, recursively |
| `maintainers/*` | Non-hidden subjects directly under `maintainers/` |
| `maintainers/**` | Non-hidden subjects under `maintainers/`, recursively |
| `ast-grep/rules` | Exact match — loads hidden subject |
| `maintainers/j*` | Non-hidden subjects starting with `j` under `maintainers/` |

Key rules:

- `*` matches files at the current level (non-recursive)
- `**` traverses directories recursively
- Globs never match hidden subjects
- Hidden subjects require an exact slug
- Disabled subjects are excluded from all matches, including exact

#### No De-duplication Across Calls

There is no mechanism to prevent the assistant from calling `learn` with the
same subject twice. System prompt subjects (via `learned`) ARE excluded from the
tool, because the system prompt is never compacted. But subjects learned via
tool calls may have been compacted away from the context window, so re-learning
them is valid.

### CLI Integration

#### The `-k` / `--knowledge` Flag

A convenience flag on the `query` command for pre-loading subjects into the
system prompt:

```sh
jp query -k "project/maintainers/*" "Review this PR"
```

Equivalent to:

```sh
jp query --cfg kb.topic.project.learned+="maintainers/*" "Review this PR"
```

**Behavior:**

- Format: `topic_id/glob_pattern`
- Repeatable: `-k "project/*" -k "skills/ast-grep"`
- Merges with existing `learned` patterns (does not replace)
- Affects the current conversation going forward

#### Config Load Paths

Pre-loading can also be configured via `config_load_paths`:

```toml
# .jp/config.d/kb/maintainers.toml
[kb.topic.project]
learned = ["maintainers/*"]
```

```bash
jp query --cfg kb/maintainers "Review this PR"
```

This leverages the existing configuration loading system without any new
infrastructure.

#### Implementation

```rust
// In Query CLI args (jp_cli)
/// Pre-load knowledge base subjects into the system prompt.
///
/// Format: `<topic>/<subject glob pattern>`. Repeatable.
#[arg(short = 'k', long = "knowledge")]
knowledge: Vec<String>,
```

Each value is split on the first `/` into `(topic_id, pattern)` and converted to
a config merge on `kb.topic.<id>.learned`.

### File Format Handling

Subject files are read as UTF-8 text. The file extension determines presentation
format.

#### Pass-through Formats

Included as-is in tool output:

| Extension | Format |
|-----------|--------|
| `.md` | Markdown |
| `.txt` | Plain text |
| `.text` | Plain text |
| (none) | Plain text |

#### Fenced Code Block Formats

Wrapped in fenced code blocks with the extension as language:

| Extension | Language tag |
|-----------|-------------|
| `.toml` | `toml` |
| `.json` | `json` |
| `.yaml` / `.yml` | `yaml` |
| `.rs` | `rust` |
| `.py` | `python` |
| `.js` | `javascript` |
| `.ts` | `typescript` |
| (other) | extension name |

Example: a `.toml` file is returned as:

````
```toml
[package]
name = "example"
```
````

#### Binary File Detection

If the first 8192 bytes of a file contain a null byte, the file is treated as
binary and skipped. The `learn` tool returns a message indicating the file was
skipped.

### Data Flow

#### System Prompt Construction

```
Config Loading (layered merge)
     │
     │ AppConfig { kb: KnowledgeBaseConfig { topics } }
     │
     ▼
Query::run
     │
     ├─── For each enabled topic:
     │    ├── Scan subjects directory
     │    ├── Filter disabled subjects
     │    ├── Identify hidden subjects
     │    ├── Match `learned` glob patterns
     │    └── Read learned subject files
     │
     ├─── Build <knowledge> section
     │    ├── Pre-loaded subjects (expanded content)
     │    └── Available topics list (for `learn` tool)
     │
     ├─── Append <knowledge> to system prompt
     │
     ├─── Generate `learn` tool definition
     │    └── Dynamic description from topic IDs/titles
     │
     └─── Register `learn` as builtin tool
```

#### `learn` Tool Execution

```
Assistant calls: learn(topic: "skills", subjects: ["ast-grep"])
     │
     ▼
ToolSource::Builtin
     │
     ├── Host resolves topic "skills" → TopicConfig
     │
     ├── Host calls jp_tool_learn::execute():
     │   ├── Passes topic config (subjects path, disabled, learned)
     │   └── Passes tool arguments (subjects: ["ast-grep"])
     │
     ▼
jp_tool_learn (native Rust, v1)
     │
     ├── Resolve subjects directory from topic config
     │
     ├── Match glob "ast-grep" against directory contents
     │   ├── Exclude hidden (unless exact match)
     │   └── Exclude disabled
     │
     ├── Read matched file(s)
     │   └── Apply format handling
     │
     └── Return Outcome::Success(formatted content)
     │
     ▼
Host returns tool result to LLM
```

### Crate Changes

#### `jp_config`

New module `src/kb.rs` with `KnowledgeBaseConfig` and `TopicConfig`, including
partial config types, merging, and CLI assignment. New `kb` field on `AppConfig`.

#### `jp_tool_learn`

New crate at `crates/jp_tool_learn/`. In v1, a regular Rust library called from
the host. Contains the `learn` tool logic: directory scanning, hidden/disabled
filtering, glob matching, file reading, format handling, and output formatting.

The public API is designed as a pure function:

```rust
// crates/jp_tool_learn/src/lib.rs

pub struct LearnInput {
    /// Absolute path to the topic's subjects directory.
    pub subjects_dir: PathBuf,
    /// Topic metadata.
    pub title: Option<String>,
    pub description: Option<String>,
    pub disabled: Vec<String>,
    pub learned: Vec<String>,
    /// Tool arguments from the LLM.
    pub topic: String,
    pub subjects: Option<Vec<String>>,
}

pub fn execute(input: LearnInput) -> jp_tool::Outcome {
    // Pure function: reads files, returns formatted content.
    // No dependency on jp_config, jp_llm, or jp_cli.
}
```

The host (`jp_llm`) constructs `LearnInput` from the resolved `TopicConfig` and
the LLM's tool arguments, then calls `execute()`.

#### `jp_cli`

- Build `<knowledge>` section during `Query::run`
- Generate and register `learn` tool definition (dynamic description)
- Add `-k` / `--knowledge` CLI flag to `Query` args
- Parse `-k` values into config overrides on `kb.topic.<id>.learned`

#### `jp_llm`

- Implement `ToolSource::Builtin` in `ToolDefinition::new()` and
  `ToolDefinition::execute()` (currently `todo!()`)
- v1: Call `jp_tool_learn::execute()` directly (native Rust)
- v2: Delegate to Wasm runtime (see [Wasm Tools](../architecture/wasm-tools.md))

#### `jp_conversation`

No changes. The `learn` tool produces standard `ToolCallRequest` /
`ToolCallResponse` events in the conversation stream.

## Drawbacks

- **Config surface area.** Adds a new top-level `kb` field with nested topic
  configuration, increasing the amount of config a user needs to understand.
  Mitigated by sane defaults (only `subjects` is required).

- **System prompt growth.** Pre-loaded subjects (`learned`) expand inline into
  the system prompt. Large or numerous pre-loaded subjects consume context
  window before the conversation starts. Users must manage this tradeoff
  themselves.

- **Discovery depends on descriptions.** The assistant only knows what topics
  contain based on the `title`, `introduction`, and `description` fields. Poor
  metadata leads to the assistant not knowing when to call `learn`. This is a
  garbage-in, garbage-out problem.

- **No search.** The `learn` tool requires knowing (or guessing) subject slugs.
  For topics with many subjects, the assistant must first list subjects, then
  load specific ones — a two-step interaction that consumes tool calls and
  tokens.

## Alternatives

### Use attachments for knowledge

The existing attachment system (`-a`) can load files per query. However,
attachments are designed for ephemeral, query-specific context. They lack:

- Structured organization (topics, slugs)
- On-demand retrieval (the assistant cannot request more)
- Persistent configuration across queries

KB and attachments serve different purposes: attachments are "here's context for
this specific question," KB is "here's what the project knows."

### Use MCP resources

MCP resource subscriptions could expose knowledge files. This adds complexity
(an MCP server to manage), external dependencies, and doesn't integrate
naturally with JP's config system. The `learn` tool is simpler and self-contained.

### Embed everything in the system prompt

For small projects this works. It breaks down when knowledge exceeds a few
thousand tokens — wasted context window on irrelevant knowledge, slower
responses, and higher cost per query. The KB's on-demand retrieval avoids
this.

### RAG / vector search

Embedding-based retrieval (vector store, semantic search) is a different
category of solution — heavier infrastructure, requires an embedding model,
and adds latency. The KB is intentionally simple: files on disk, glob patterns,
structured metadata. If a project needs semantic search, it can run an MCP
server for that. The KB serves the common case where the user knows what
knowledge exists and wants to organize it for the assistant.

## Non-Goals

- **Semantic search or indexing.** The KB is file-based with explicit
  addressing. No embeddings, no full-text search, no indexing.
- **Cross-workspace sharing.** Topics are scoped to the workspace. Sharing
  knowledge between workspaces is out of scope (users can symlink or use shared
  config).
- **Versioning or change tracking.** Subject files are plain files in the
  workspace. Version control is handled by Git, not by JP.
- **Write access.** The `learn` tool is read-only. A future `memory` tool
  (see [#102]) may add write capabilities. This RFD does not cover that.
- **Wasm execution.** The v1 implementation runs `jp_tool_learn` as native
  Rust. Migration to Wasm is described in the [Wasm Tools] architecture
  document and is deferred to a later phase.

## Risks and Open Questions

- **LLM glob literacy.** The tool accepts glob patterns (`*`, `**`). Some LLMs
  may not understand glob semantics well and could produce invalid patterns.
  Mitigated by clear tool description text and the fallback of exact slug
  matching.

- **Large subject files.** No size limit is enforced on subject files. A single
  large file could blow out the context window. Should we warn or truncate above
  a threshold? For now, this is the user's responsibility.

- **Topic discovery at scale.** With many topics, the `<knowledge>` system
  prompt section grows. If a workspace defines 20+ topics, the topic listing
  itself becomes noise. This is unlikely in practice but worth monitoring.

- **`ToolSource::Builtin` implementation.** The `learn` tool is the first
  builtin tool. The `ToolSource::Builtin` code path in `jp_llm` is currently
  `todo!()`. The implementation needs to handle tool definition generation,
  argument passing, and result handling — all of which are new for this source
  type.

## Implementation Plan

### Phase 1: Configuration

1. Create `jp_config/src/kb.rs` with `KnowledgeBaseConfig` and `TopicConfig`
2. Add partial config types, merging, and CLI assignment
3. Add `kb` field to `AppConfig`
4. Add config snapshot tests

Can be merged independently.

### Phase 2: System Prompt Injection

1. Implement subject directory scanning and slug computation
2. Implement hidden/disabled filtering
3. Implement `learned` glob matching and file reading
4. Implement file format handling (pass-through vs fenced)
5. Build `<knowledge>` section generator
6. Integrate into `Query::run` system prompt construction
7. Add unit tests for section generation

Depends on Phase 1.

### Phase 3: `learn` Tool Definition

1. Implement schema generation with dynamic description
2. Implement topic resolution (ID and title matching)
3. Register `learn` as a builtin tool (conditional on KB config)
4. Add `-k` / `--knowledge` flag to `Query` CLI args
5. Add unit tests for schema generation and topic resolution

Depends on Phase 1. Can proceed in parallel with Phase 2.

### Phase 4: `learn` Tool Execution (v1 — native Rust)

1. Create `jp_tool_learn` crate as a regular Rust library
2. Implement `LearnInput` and `execute()` as a pure function
3. Implement listing logic (directory scan, hidden/disabled filter)
4. Implement loading logic (glob matching, file reading, format handling)
5. Implement `ToolSource::Builtin` in `jp_llm` — construct `LearnInput`
   from `TopicConfig` + LLM arguments, call `jp_tool_learn::execute()`
6. Add integration tests (tool call → file content)

Depends on Phases 2 and 3.

### Phase 5: Testing

1. Unit tests: config parsing, slug resolution, glob matching, format handling,
   section generation, schema generation
2. Integration tests: full `learn` tool call (native execution)
3. Edge cases: empty topics, all-learned topics, all-disabled subjects, binary
   files, deeply nested directories

Runs alongside Phase 4.

## References

- [Issue #102: Long-term memory system][#102] — broader vision for
  cross-conversation knowledge
- [Wasm Tools Architecture][Wasm Tools] — target Wasm plugin infrastructure
  that `learn` will migrate to in v2

[#102]: https://github.com/dcdpr/jp/issues/102
[Wasm Tools]: ../architecture/wasm-tools.md
