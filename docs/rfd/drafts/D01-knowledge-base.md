# RFD D01: Knowledge Base

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23
- **Summary**: Organizes project knowledge into configured topics and subjects
  retrieved on demand via a `learn` tool or `-k` flag.

## Summary

This RFD proposes a knowledge base (KB) system that organizes reusable,
project-specific knowledge into configured directories of **topics** and
**subjects**.
The assistant discovers available topics in the system prompt and retrieves
subjects on demand with a built-in `learn` tool.
Users can also pre-load subjects into the system prompt through configuration or
per query with `-k`.

## Motivation

JP's assistant operates with whatever context it receives: the system prompt,
conversation history, attachments, and tool call results.
For project-specific knowledge (coding conventions, architecture decisions, team
information, skill guides), users have two options:

1. **Inline everything in the system prompt.** This works for small amounts of
   knowledge but does not scale.
   A system prompt stuffed with every convention and guide wastes context window
   on material the assistant never uses for a given query.

2. **Attach files per query.** The attachment system (`-a`) loads files into
   each query, but attachments target ephemeral, query-specific context such as
   a file to review or command output to analyze.
   They provide no structure for organizing persistent project knowledge, and
   the assistant cannot discover or request attachments on its own.

> REVIEW:
>
> Attachments are also added to the system prompt; they are a convenient way to
> attach content fetched by a handler (such as `file://` or `https://`), making
> them similar to inlining everything in the system prompt.

Neither option lets the assistant discover available knowledge and fetch only
what a query requires.
Without a dedicated mechanism, projects with substantial domain knowledge must
either bloat the system prompt or attach files by hand on every query.
The knowledge base addresses this with a file-based catalog: the assistant sees
a menu of topics in the system prompt and loads individual subjects on demand.

## Design

### User-Facing Behavior

A user configures topics in their workspace configuration.
Each topic points to a directory of text files (subjects):

```toml
[kb.topic.project]
title = "General Project Knowledge"
introduction = "foo bar baz..."
subjects = ".jp/kb/project"

[kb.topic.skills]
title = "Learnable Assistant Skills"
subjects = ".jp/kb/skills"
```

> REVIEW:
>
> I wonder if we should move `kb` into the `assistant` section, to avoid
> overwhelming the root level of the config.

The assistant sees available topics in its system prompt and retrieves subjects
by calling the `learn` tool.
Users can also pre-load subjects into the system prompt per query or through
configuration:

```sh
# Pre-load via CLI flag
jp query -k "project/maintainers/*" "Review this PR"

# Or via config
jp query --cfg kb.topic.project.learned+="maintainers/*" "Review this PR"
```

### Design Goals

| Goal                     | Description                                              |
| ------------------------ | -------------------------------------------------------- |
| **Structured knowledge** | Organize knowledge into topics and subjects              |
| **On-demand retrieval**  | Fetch subjects on demand via the `learn` tool            |
| **Hidden subjects**      | Exclude `.`-prefixed paths from default listings         |
| **Pre-loaded subjects**  | Inject selected subjects directly into the system prompt |
| **Glob support**         | Match multiple subjects with glob patterns               |

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

| Field          | Required | Default | Description                            |
| -------------- | -------- | ------- | -------------------------------------- |
| `enable`       | No       | `true`  | Set `false` to deactivate the topic    |
| `title`        | No       | `-`     | Display name shown to the assistant    |
| `introduction` | No       | `-`     | One-line summary in system prompt      |
| `description`  | No       | `-`     | Extended description in `learn` output |
| `subjects`     | **Yes**  | `-`     | Path to subject directory              |
| `learned`      | No       | `[]`    | Glob patterns for pre-loaded subjects  |
| `disabled`     | No       | `[]`    | Excluded subject slugs                 |

### Subject Resolution

A **subject** is a text file inside a topic's `subjects` directory.
The **slug** is the file path relative to the topic directory, with the file
extension stripped.

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

| Path                     | Hidden? | Reason                           |
| ------------------------ | ------- | -------------------------------- |
| `.internal-notes.md`     | Yes     | Filename starts with `.`         |
| `ast-grep/.rules.md`     | Yes     | Nested filename starts with `.`  |
| `.hidden-dir/visible.md` | Yes     | Parent directory starts with `.` |
| `maintainers/jean.md`    | No      | No `.`-prefixed components       |

Hidden subjects are:

- **Omitted from listings**: `learn` never lists them, and `*` or `**` globs do
  not match them.
- **Loadable by exact slug**: `learn(topic, subjects: ["ast-grep/rules"])` loads
  the subject directly.
- **Discovered through references**: a visible subject can point the assistant
  to a hidden slug (for example, "read `ast-grep/rules` for full rule
  documentation").

#### Disabled Subjects

Subjects matching an entry in `disabled` are excluded completely: `learn`
neither lists nor loads them, even by exact slug, and `disabled` takes
precedence over `learned`.

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

When at least one enabled topic has subjects that are neither hidden, disabled,
nor pre-loaded via `learned`, the system prompt includes a `<knowledge>`
section:

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

Each entry in the topic list follows three rules:

1. Include only enabled topics that have at least one non-hidden, non-disabled
   subject not already matched by `learned`.
2. Show `title` in bold after the topic ID when set.
3. Append `introduction` after the title when set.

#### Pre-loaded Knowledge

Subjects matched by a topic's `learned` patterns are expanded inline in
`<knowledge>`:

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

Pre-loaded subjects are omitted from the available subjects list in `learn`.
Calling `learn` without `subjects` lists them under a separate "Already learned"
heading so the assistant knows they are already in context.

#### Why Not Configurable Sections

`SectionConfig` (content, tag, position) defines user-configured system prompt
sections.
The `<knowledge>` section is derived from the KB configuration rather than
configured directly, so it is built programmatically during thread construction.
The implementation may still use `SectionConfig` internally as a container (with
a fixed `"knowledge"` tag and a low position value), without exposing it as a
user-facing section setting.

#### Injection Point

`Query::run` builds the `<knowledge>` section after resolving the configuration
and before constructing the LLM request:

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

`learn` is registered via `ToolSource::Builtin` when at least one enabled topic
has subjects that are neither pre-loaded nor disabled.
Its definition is generated at query time from the resolved KB configuration:

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

#### JSON Schema

The `topic` parameter is a free-form string whose description is generated from
the KB configuration to list available topic IDs and titles.
Keeping the schema structure constant across KB configuration changes avoids
provider issues with cached tool schemas between turns:

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

#### Topic Resolution

The tool resolves the `topic` argument in order:

1. Exact match on the topic ID.
2. Case-insensitive match on the topic `title`.
3. Otherwise, return an error listing the valid topics.

#### Listing Subjects

Calling `learn` without `subjects` returns the topic metadata and lists its
available subjects:

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

#### Loading Subjects

Loading a single subject returns the formatted content of
`.jp/kb/skills/ast-grep.md` (see [File Format Handling](#file-format-handling)):

```js
learn(topic: "skills", subjects: ["ast-grep"])
```

Loading multiple subjects or a glob returns all matching non-hidden subjects,
each wrapped in a `<subject>` tag:

```js
learn(topic: "skills", subjects: ["**"])
```

```xml
<subject "ast-grep">
...content...
</subject>
```

#### Glob Matching

`subjects` patterns follow standard glob semantics: `*` matches within a single
directory level, and `**` matches recursively across directories.
Globs never match hidden subjects; loading a hidden subject requires its exact
slug.
Disabled subjects are excluded from both glob and exact matches.

| Pattern          | Matches                                                    |
| ---------------- | ---------------------------------------------------------- |
| `*`              | Non-hidden subjects at the top level only                  |
| `**`             | All non-hidden subjects, recursively                       |
| `maintainers/*`  | Non-hidden subjects directly under `maintainers/`          |
| `maintainers/**` | Non-hidden subjects under `maintainers/`, recursively      |
| `ast-grep/rules` | Exact match (loads hidden subject)                         |
| `maintainers/j*` | Non-hidden subjects starting with `j` under `maintainers/` |

#### Repeated `learn` Calls

`learn` does not deduplicate subjects across tool calls.
Subjects pre-loaded via `learned` stay in the system prompt, which is never
compacted, so `learn` excludes them.
Subjects loaded through earlier tool calls can be compacted out of the
conversation history, so the assistant is allowed to load them again.

### CLI Integration

#### The `-k` / `--knowledge` Flag

`jp query` accepts `-k` / `--knowledge` as a shorthand for pre-loading subjects
into the system prompt:

```sh
jp query -k "project/maintainers/*" "Review this PR"
```

This is equivalent to:

```sh
jp query --cfg kb.topic.project.learned+="maintainers/*" "Review this PR"
```

Each flag value has the form `topic_id/glob_pattern` and can be repeated (for
example, `-k "project/*" -k "skills/ast-grep"`).
Patterns append to the topic's existing `learned` list and persist for the rest
of the conversation.

#### Config Load Paths

Reusable sets of pre-loaded subjects can also live in `config_load_paths`:

```toml
# .jp/config.d/kb/maintainers.toml
[kb.topic.project]
learned = ["maintainers/*"]
```

```bash
jp query --cfg kb/maintainers "Review this PR"
```

#### Implementation

```rust
// In Query CLI args (jp_cli)
/// Pre-load knowledge base subjects into the system prompt.
///
/// Format: `<topic>/<subject glob pattern>`. Repeatable.
#[arg(short = 'k', long = "knowledge")]
knowledge: Vec<String>,
```

Each `-k` value is split on the first `/` into `(topic_id, pattern)` and merged
into `kb.topic.<id>.learned`.

### File Format Handling

Subject files are read as UTF-8 text, and the file extension determines how
their content is formatted.

#### Pass-through Formats

Markdown and plain-text files are included as-is in tool output:

| Extension | Format     |
| --------- | ---------- |
| `.md`     | Markdown   |
| `.txt`    | Plain text |
| `.text`   | Plain text |
| (none)    | Plain text |

#### Fenced Code Block Formats

Other text files are wrapped in a fenced code block tagged with the language for
their extension:

| Extension        | Language tag   |
| ---------------- | -------------- |
| `.toml`          | `toml`         |
| `.json`          | `json`         |
| `.yaml` / `.yml` | `yaml`         |
| `.rs`            | `rust`         |
| `.py`            | `python`       |
| `.js`            | `javascript`   |
| `.ts`            | `typescript`   |
| (other)          | extension name |

For example, a `.toml` file is returned as:

````
```toml
[package]
name = "example"
```
````

#### Binary File Detection

If the first 8192 bytes of a file contain a null byte, `learn` treats the file
as binary, skips it, and returns a message indicating that the file was skipped.

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

Add `src/kb.rs` defining `KnowledgeBaseConfig` and `TopicConfig` (with partial
config types, merging, and CLI assignment), and add `kb` to `AppConfig`.

#### `jp_tool_learn`

Create `crates/jp_tool_learn/` as a native Rust library called by the host in
v1.
It implements directory scanning, hidden and disabled subject filtering, glob
matching, file reading, format handling, and output formatting, exposed through
a pure function:

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
- v2: Delegate to Wasm runtime (see [Wasm Tools])

#### `jp_conversation`

`jp_conversation` requires no changes; `learn` produces standard
`ToolCallRequest` and `ToolCallResponse` events in the conversation stream.

## Drawbacks

- **Config surface area.** Adds a top-level `kb` section with nested topic
  settings.
  Requiring only `subjects` keeps the minimal configuration small.

- **System prompt growth.** Pre-loaded subjects (`learned`) expand inline into
  the system prompt, consuming context window before the conversation starts.
  Users must balance what they pre-load against what stays on demand.

- **Discovery depends on descriptions.** The assistant decides when to call
  `learn` from each topic's `title`, `introduction`, and `description`.
  Vague metadata leaves the assistant unable to tell when a topic is relevant.

- **No search.** `learn` addresses subjects by slug or glob rather than search
  query.
  In topics with many subjects, the assistant must list subjects in one tool
  call and load them in a second, spending an extra round-trip and tokens.

## Alternatives

### Use Attachments for Knowledge

The attachment system (`-a`) loads files into a query, but attachments target
ephemeral, query-specific context.
They have no topic or slug hierarchy, cannot be fetched on demand by the
assistant, and are not organized as a persistent catalog across queries.

### Use MCP Resources

MCP resource subscriptions could expose knowledge files, but doing so requires
running an external MCP server and does not integrate with JP's layered
configuration.
A built-in `learn` tool is self-contained.

### Embed Everything in the System Prompt

Inlining all project knowledge into the system prompt works for small projects,
but once knowledge exceeds a few thousand tokens it wastes context window on
unrelated material, increases latency, and raises per-query cost.

### RAG / Vector Search

Embedding-based retrieval requires a vector store and an embedding model, adding
infrastructure and query latency.
Projects that need semantic search can expose it through an MCP server; the KB
covers the common case where users already organize knowledge as files on disk.

## Non-Goals

- **Semantic search or indexing.** The KB addresses files by topic and slug
  without embeddings, full-text search, or indexing.

- **Cross-workspace sharing.** Topics are scoped to a single workspace; users
  who want shared knowledge across workspaces can use symlinks or shared
  configuration files.

- **Versioning or change tracking.** Subject files are plain workspace files
  tracked by Git rather than JP.

- **Write access.** `learn` is read-only.
  Write capabilities belong to a future `memory` tool (see [#102]).

- **Wasm execution.** v1 runs `jp_tool_learn` as native Rust and defers Wasm
  execution to a later phase.

  > [!TIP]
  > [RFD 016] defines the Wasm plugin infrastructure that `learn` would migrate
  > to, with [RFD 017] covering the first capability interface built on it.

## Risks and Open Questions

- **LLM glob literacy.** Models that misapply `*` and `**` may pass invalid or
  overly broad patterns.
  The parameter description explains both wildcards, and exact slug matching
  works without globs.

- **Large subject files.** Subject files have no size limit, so a single large
  file can fill the context window.
  Whether JP should warn or truncate above a size threshold remains open; for
  v1, sizing subjects is the user's responsibility.

- **Topic discovery at scale.** Defining 20+ topics bloats the `<knowledge>`
  listing in the system prompt.

- **`ToolSource::Builtin` implementation.** `learn` is the first built-in tool,
  and the `ToolSource::Builtin` path in `jp_llm` is currently `todo!()`.
  It requires wiring up tool definition generation, argument passing, and result
  handling.

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

Depends on Phase 1.
Can proceed in parallel with Phase 2.

### Phase 4: `learn` Tool Execution (v1, Native Rust)

1. Create `jp_tool_learn` crate as a regular Rust library
2. Implement `LearnInput` and `execute()` as a pure function
3. Implement listing logic (directory scan, hidden/disabled filter)
4. Implement loading logic (glob matching, file reading, format handling)
5. Implement `ToolSource::Builtin` in `jp_llm` to construct `LearnInput` from
   `TopicConfig` and LLM arguments and call `jp_tool_learn::execute()`
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

- [Issue #102: Long-term memory system][#102]: broader vision for
  cross-conversation knowledge
- [Wasm Tools Architecture][Wasm Tools]: target Wasm plugin infrastructure that
  `learn` migrates to in v2

[#102]: https://github.com/dcdpr/jp/issues/102
[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 017]: ../017-wasm-attachment-handlers.md
[Wasm Tools]: ../../wiki/00dxyb9-wasm-tools-architecture.md
