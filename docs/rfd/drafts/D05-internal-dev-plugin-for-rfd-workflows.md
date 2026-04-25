# RFD D05: Internal Dev Plugin for RFD Workflows

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20

## Summary

This RFD introduces `jp-dev`, an internal command plugin that provides
structured, AI-assisted workflows for the full RFD lifecycle: authoring new
RFDs through a phased explore → converge → draft process, reviewing and
refining drafts, and executing implementation plans phase by phase. The plugin
manages a state directory alongside the RFD documents, orchestrates `jp`
conversations for each workflow step with appropriate model and tool selection,
and carries context (research summaries, locked decisions, reviews, phase
summaries) across steps so that each AI interaction is focused and
well-informed.

## Motivation

The existing RFD tooling (`just rfd-*`) handles lifecycle transitions: creating
drafts, promoting status, superseding, and abandoning. What it does not handle
is the *work between transitions* — the iterative process of exploring a
problem space, converging on design decisions, drafting the document
section-by-section, reviewing and refining the draft, and executing the
implementation plan phase by phase.

Today this work happens through ad-hoc `jp query` invocations. The contributor
manually assembles the right persona, model, attachments, and prompt for each
step. Research done in one conversation is not carried into the next. Design
decisions made during a brainstorming session live only in conversation history.
Reviews are not preserved as artifacts. Implementation summaries from one phase
are not automatically carried into the next. There is no structured way to see
"where am I in this RFD's lifecycle?"

The authoring gap is especially costly. Complex RFDs benefit from a phased
process: cheap, fast models for initial research (reading code, scanning prior
art, searching crates); strong models for design convergence (evaluating
trade-offs, locking decisions); and careful section-by-section drafting with
explicit review gates. Today, contributors do all of this in a single
conversation with one model and one tool set — or worse, across several
disconnected conversations where context is lost between sessions.

The `dev` plugin automates this orchestration. Each workflow step is a
subcommand that:

1. Selects the appropriate model and tool set for the task.
2. Attaches relevant artifacts from prior steps as context.
3. Opens `$EDITOR` so the contributor can steer the interaction.
4. Saves the output as a durable artifact.
5. Feeds that artifact into subsequent steps.

The plugin is internal to the JP project — it is specific to how JP itself is
built and is not intended for general distribution.

## Design

### User Experience

The entry point is `jp dev rfd`. The status display adapts to the RFD's current
state in the workflow:

**During authoring** (before the RFD document is complete):

```
$ jp dev rfd D32
RFD D32: JP Tracing Infrastructure

Status: Draft (authoring)
Explore: complete (explore.md)
Decisions: 13 locked, 0 discussing
Sections: 2/8 drafted (Summary, Motivation)

Actions:
  jp dev rfd D32 explore     Re-run research (sonnet)
  jp dev rfd D32 converge    Continue decision discussion (opus)
  jp dev rfd D32 draft       Draft next section (opus)
```

**During review** (RFD document exists, under discussion):

```
$ jp dev rfd 045
RFD 045: Layered Interrupt Handler Stack

Status: Discussion
Reviews: 2 active (reviews/1721500000.md, reviews/1721586000.md)

Actions:
  jp dev rfd 045 review    Review the RFD (default: gemini-pro)
  jp dev rfd 045 refine    Refine the RFD based on reviews (default: opus)
  jp dev rfd 045 accept    Extract plan and promote to Accepted
```

**During implementation** (after acceptance and plan extraction):

```
$ jp dev rfd 045
RFD 045: Layered Interrupt Handler Stack

Status: Accepted
Phases:
  [x] 1. Interrupt handler trait extraction (phases/1.md)
  [ ] 2. Stack-based handler registration
  [ ] 3. Per-tool interrupt policies

Actions:
  jp dev rfd 045 implement   Implement the next phase (default: opus)
  jp dev rfd 045 review      Review the implementation so far
  jp dev rfd 045 refine      Refine the implementation
  jp dev rfd 045 done        Promote to Implemented
```

The plugin accepts either bare numbers (`045`, `45`) or draft IDs (`D32`). It
resolves the RFD file by globbing `docs/rfd/045-*.md` or `docs/rfd/D32-*.md`.

### State Directory

Workflow state lives in `docs/rfd/.state/<NNN>/`:

```
docs/rfd/.state/
└── 045/
    ├── explore.md
    ├── decisions.md
    ├── plan.json
    ├── reviews/
    │   ├── 1721500000.md
    │   └── 1721586000.md
    ├── phases/
    │   ├── 1.md
    │   └── 2.md
    └── impl-reviews/
        └── 1721600000.md
```

The directory is gitignored. All state is derived from filesystem presence:

| State               | Source                                      |
|---------------------|---------------------------------------------|
| RFD status          | Parsed from the `- **Status**: ...` line    |
| Explore complete    | `explore.md` file presence                  |
| Decisions exist     | `decisions.md` file presence                |
| Plan exists         | `plan.json` file presence                   |
| Phase count         | `plan.json` array length                    |
| Completed phases    | Which `phases/N.md` files exist             |
| Active reviews      | Files in `reviews/`                         |
| Active impl reviews | Files in `impl-reviews/`                    |

No `state.json`. The filesystem *is* the state. Recovery from a broken state
is trivial — delete or add a file.

### Workflow Steps

Each step spawns a `jp` subprocess that inherits the terminal so the
contributor gets the full interactive experience: `$EDITOR` for composing the
prompt, streaming markdown rendering, tool call display, and so on.

After the subprocess completes, the plugin extracts the assistant's final
response via the plugin protocol. It calls `read_events` for the conversation
(identified by the conversation ID obtained during creation), finds the last
`chat_response` event, and saves the message content as a Markdown file.

#### Creating conversations

Each workflow step creates a dedicated conversation for its interaction. The
plugin uses `jp conversation new` ([RFD 050]) to create the conversation and
capture its ID, then runs `jp query --id=<ID>` against it. This keeps the
contributor's active conversation unchanged.

Before RFD 050 is implemented, the plugin falls back to parsing the
conversation ID from `jp query --new` output, or identifies the most recently
created conversation in the workspace after the subprocess exits.

### Author Workflow

The author workflow covers the work *before* the RFD document exists. It
manages three phases with distinct model and tool profiles, each producing a
durable artifact that feeds into the next.

The full lifecycle including the author workflow:

```
explore → converge → draft → review → refine → accept → implement → done
```

The author workflow covers `explore`, `converge`, and `draft`. The remaining
steps (`review` through `done`) are unchanged from the original design.

#### `explore`

Research the problem space with a fast, cheap model.

```
jp dev rfd author "Better tracing for JP"
jp dev rfd D32 explore                    # re-run after draft exists
jp dev rfd D32 explore --model=flash       # override model
```

- **Model**: sonnet (fast, cheap — this is research and brainstorming).
- **Tools**: read-only (`fs_read_file`, `fs_grep_files`, `fs_list_files`,
  `github_issues`, `github_pulls`), web access (`web_fetch`), crate research
  (`crates_search`, `crate_readme`, `crate_search_items`). No write tools.
- **Attaches**: the contributor's problem statement or notes.
- **Behavior**: the assistant investigates the codebase, reads related RFDs,
  searches for crates and prior art, asks clarifying questions. The
  conversation is freeform.
- **Artifact**: after the conversation ends, the plugin extracts the
  assistant's final response and saves it to `explore.md`.

When invoked as `jp dev rfd author "title"`, the plugin creates a new state
directory, records the title, and immediately enters the explore phase. When
invoked as `jp dev rfd D32 explore`, it re-runs research for an existing
draft, replacing the previous `explore.md`.

The contributor can run `explore` multiple times to deepen research. Each run
replaces the explore artifact.

#### `converge`

Converge on design decisions with a strong model.

```
jp dev rfd D32 converge
```

- **Model**: opus (deep reasoning — this is where design decisions get made).
- **Tools**: read-only tools (same as explore), plus the `rfd_decision` tool
  (see [Dedicated RFD Tools](#dedicated-rfd-tools)). No general file write
  access.
- **Attaches**: the explore artifact (`explore.md`), the original problem
  statement, RFD 001 (the process document).
- **Behavior**: the assistant proposes options, the contributor pushes back,
  they iterate. The assistant uses the `rfd_decision` tool to record each
  decision as it is made — adding new decisions, updating existing ones, or
  marking them as rejected. The `decisions.md` file evolves through the
  conversation as the living record of the design.
- **Artifact**: `decisions.md`, maintained incrementally via `rfd_decision`
  tool calls throughout the conversation.

The `rfd_decision` tool runs in `ask` mode — the contributor confirms each
decision write. This provides a natural checkpoint: the contributor reviews
each decision as it is recorded and can reject or revise it before it lands
in the file.

The conversation continues as long as needed. There is no forced end. When
the contributor is satisfied, they end the conversation naturally.

The state machine enforces ordering: `draft` is not available until
`decisions.md` exists. The contributor can re-enter `converge` at any time to
revise decisions, even after drafting has started.

##### Decision file format

The `rfd_decision` tool maintains `decisions.md` with consistent structure:

```markdown
# Decisions for RFD D32: JP Tracing Infrastructure

## Locked

1. Flat event structs in per-crate `trace::events` modules, `pub(crate)`.
2. `emit!` macro captures `file!`/`line!`, invokes test recorder.
3. Caller location always recorded as fields.

## Under Discussion

4. Chrome verbosity API shape.

## Rejected

- ~~Enum-based event hierarchy.~~ Spans carry namespace context instead.
```

#### `draft`

Draft the RFD document section-by-section with explicit review gates.

```
jp dev rfd D32 draft
```

- **Model**: opus.
- **Tools**: read-only tools, plus `rfd_draft` (to create the file) and
  `rfd_section` (to write individual sections). See [Dedicated RFD
  Tools](#dedicated-rfd-tools). No general file write access.
- **Attaches**: `decisions.md`, `explore.md`, RFD 001.
- **Behavior**: the assistant follows a two-stage confirmation process for
  each section:

  1. **State intent.** The assistant announces which section it will write
     next, describes its plan (what points it will cover, how it will frame
     them, what it is deliberately excluding), and waits for approval.
  2. **Contributor reviews intent.** Agrees, pushes back, or redirects.
  3. **Write the section.** The assistant calls `rfd_section` with the
     content. The tool runs in `ask` mode — the contributor sees the write
     and confirms. Because they already reviewed the intent in step 1, they
     can correlate the written content against the agreed plan and reject if
     they don't match.
  4. **Proceed to next section.** The assistant does NOT continue to the next
     section until the contributor confirms.

- **Artifact**: the RFD file itself, created by `rfd_draft` and populated by
  `rfd_section`.

The system prompt encodes the two-stage confirmation rule:

> Before writing each section: (1) state which section you will write next,
> (2) describe your plan — what points you will cover, how you will frame
> them, and what you are deliberately excluding, (3) wait for approval before
> writing, (4) do not proceed to the next section until the contributor
> confirms.

If the contributor is unsatisfied with a section after it is written, they
can ask the assistant to revise it within the same conversation. The
assistant calls `rfd_section` again for the same section, overwriting the
previous content.

### Dedicated RFD Tools

The author workflow uses purpose-built tools instead of raw `fs_modify_file`.
These tools encode the workflow semantics — the LLM interacts with
"decisions" and "sections" rather than files and diffs.

#### `rfd_decision`

Manages the decisions file for the `converge` phase.

```
Parameters:
  action:  "add" | "update" | "reject"   (required)
  number:  integer                        (required for update/reject)
  text:    string                         (the decision statement)
  status:  "locked" | "discussing"        (default: "discussing")
  reason:  string                         (required for reject)
```

Internally writes to `docs/rfd/.state/<id>/decisions.md`. The tool manages
the file format — the LLM never touches the raw markdown directly. Each call
is confirmed by the contributor in `ask` mode.

#### `rfd_section`

Writes a single section of the RFD file during the `draft` phase.

```
Parameters:
  section:  string    (e.g., "summary", "motivation", "design")
  content:  string    (the markdown content for this section)
```

Internally calls `fs_modify_file` on the RFD file, replacing the template
placeholder or existing content for that section. The tool knows the RFD file
path from the workflow state. Runs in `ask` mode.

#### `rfd_explore_summary`

Writes the research summary for the `explore` phase.

```
Parameters:
  content:  string    (the research summary markdown)
```

Writes to `docs/rfd/.state/<id>/explore.md`. Runs in `ask` mode.

These tools live alongside the existing `rfd_draft`, `rfd_promote`, etc. in
`.jp/mcp/tools/rfd/` and `.config/jp/tools/src/rfd/`.

### Model and Tool Selection

Each workflow step uses a model and tool set appropriate to its purpose:

| Step        | Default Model | Tools                         | Write Access        |
|-------------|---------------|-------------------------------|---------------------|
| `explore`   | sonnet        | read-only, web, crate search, | `rfd_explore_summary` (ask) |
|             |               | `rfd_explore_summary`         |                     |
| `converge`  | opus          | read-only, `rfd_decision`     | `rfd_decision` (ask)|
| `draft`     | opus          | read-only, `rfd_draft`,       | `rfd_draft` (ask),  |
|             |               | `rfd_section`                 | `rfd_section` (ask) |
| `review`    | gemini-pro    | read-only                     | none                |
| `refine`    | opus          | read-only, `rfd_section`      | `rfd_section` (ask) |
| `implement` | opus          | full dev toolset              | yes                 |

All steps accept `--model=<id>` to override the default.

#### `review`

Spawns a `jp` conversation with the architect persona and a review-focused
model (default: Gemini Pro). The RFD document is attached. The contributor
sees a pre-filled prompt in `$EDITOR` asking for a review, which they can
edit before sending.

After the conversation ends, the assistant's final response is saved to
`reviews/<timestamp>.md`.

Multiple reviews can coexist. Each is an independent conversation.

```
jp dev rfd 045 review                  # default model
jp dev rfd 045 review --model=opus     # override model
```

#### `refine`

Spawns a conversation with the dev persona (default: Opus). Attaches the RFD
document and all files in `reviews/`. The contributor steers the refinement
via `$EDITOR`.

After refinement completes, the plugin lists active reviews and prompts:

```
Active reviews:
  1. reviews/1721500000.md - "Error handling in Design section needs..."
  2. reviews/1721586000.md - "Migration path is underspecified..."

Dismiss resolved reviews? [numbers, 'all', or enter to skip]: 1
Dismissed reviews/1721500000.md
```

Dismissed reviews are deleted from `reviews/`. They have served their purpose
and no longer need to influence future refinement steps.

#### `accept`

Two operations in sequence:

1. **Plan extraction.** Spawns a `jp` conversation with structured output. The
   RFD's Implementation Plan section is parsed into a JSON array of phases:

   ```json
   [
     {
       "title": "Interrupt handler trait extraction",
       "description": "Extract..."
     },
     {
       "title": "Stack-based handler registration",
       "description": "Add..."
     },
     {
       "title": "Per-tool interrupt policies",
       "description": "Implement..."
     }
   ]
   ```

   The schema constrains the output. The model reads the RFD and extracts
   whatever phases the author defined in the Implementation Plan section.

2. **Lifecycle promotion.** Calls `just rfd-promote <NNN>` to transition the
   RFD from Discussion to Accepted. This creates the GitHub tracking issue and
   updates the metadata header per the existing process.

If plan extraction fails, the promotion does not run. The contributor can fix
the Implementation Plan section and try again.

If the RFD is in Draft status, `accept` first promotes to Discussion (assigning
a permanent number), then promotes to Accepted. Both promotions happen through
`just rfd-promote`.

#### `implement`

Determines the next unfinished phase from `plan.json` and the presence of
`phases/N.md` files. Spawns a conversation with the dev persona (default:
Opus). Attaches:

- The RFD document
- The phase description from `plan.json`
- All previous `phases/N.md` summaries (so the assistant understands what was
  already done)

The system prompt instructs the assistant to:

1. Implement the specified phase.
2. At the end, provide a brief summary of what was implemented, any deviations
   from the RFD, and notes relevant to future phases.

After the conversation ends, the assistant's final response is saved to
`phases/N.md`.

The contributor can continue the implementation conversation using `jp q` if
the first pass was incomplete. The `phases/N.md` file is only written after
the plugin-managed conversation ends, so continuing the conversation extends
the work before the summary is captured.

#### `done`

Calls `just rfd-promote <NNN>` to transition from Accepted to Implemented.
No LLM interaction.

### How the Plugin Extracts Responses

After a `jp query` subprocess finishes, the plugin needs the assistant's last
message. It uses the plugin protocol for this:

1. During the workflow step, the plugin tracks the conversation ID (obtained
   from `jp conversation new` or parsed from `jp query` output).
2. After the subprocess exits, the plugin sends a `read_events` request
   through its host protocol connection for that conversation ID.
3. The host responds with the conversation's events as JSON.
4. The plugin scans backwards for the last `chat_response` event and extracts
   the `message` field.

This keeps the plugin decoupled from the on-disk conversation format. The
protocol handles serialization and format evolution.

### Plugin Structure

```
crates/internal/dev/
├── Cargo.toml
├── src/
│   ├── main.rs            # plugin protocol, subcommand dispatch
│   ├── rfd/
│   │   ├── mod.rs         # `jp dev rfd` dispatch and status display
│   │   ├── resolve.rs     # find RFD file, parse metadata, derive state
│   │   ├── author.rs      # `author` entry point (create state, enter explore)
│   │   ├── explore.rs     # spawn explore conversation
│   │   ├── converge.rs    # spawn converge conversation
│   │   ├── draft.rs       # spawn draft conversation (section-by-section)
│   │   ├── review.rs      # spawn review conversation
│   │   ├── refine.rs      # spawn refine conversation, dismiss prompt
│   │   ├── accept.rs      # extract plan + promote
│   │   ├── implement.rs   # implement next phase
│   │   └── done.rs        # promote to Implemented
│   └── jp.rs              # helper: spawn `jp` subprocess
```

The binary is named `jp-dev` and registers the command path `["dev"]`. It is
not published to the plugin registry.

### Subprocess Spawning (`jp.rs`)

The `jp.rs` module provides a helper for spawning `jp` as a subprocess. It
constructs the argument list (persona, model, attachments, flags) and spawns
the process with inherited stdin/stdout/stderr so the contributor gets the
full terminal experience.

The helper is designed for future migration to protocol-based query delegation
([RFD D18]). The interface is a function that takes a conversation spec
(persona, model, attachments, prompt) and returns the conversation ID after
execution. Swapping the implementation from subprocess to protocol message
is a localized change behind this interface.

### Review and Refinement During Implementation

The `review` and `refine` commands adapt their behavior based on the RFD's
current status:

| Status     | `review` target    | `refine` target    | Review storage     |
|------------|--------------------|--------------------|--------------------|
| Discussion | The RFD document   | The RFD document   | `reviews/`         |
| Accepted   | The implementation | The implementation | `impl-reviews/`    |

During the Discussion phase, reviews evaluate the *design*. During the
Accepted phase, reviews evaluate the *implementation* — the plugin attaches
completed phase summaries alongside the RFD so the reviewer can assess
whether the implementation matches the design.

### Installation

The plugin is built and installed locally via the existing `just` infrastructure:

```sh
just plugin-build-local
```

This builds all command plugin binaries (including `jp-dev`) and copies them
to the local plugin directory. Since `crates/internal/*` is already in the
workspace members list, no workspace configuration changes are needed.

## Drawbacks

- **Subprocess spawning.** The plugin spawns `jp` as a child process for each
  workflow step. This means a second `jp` instance performs workspace discovery,
  config loading, and LLM interaction independently of the host `jp` that is
  running the plugin. This works but is inelegant. [RFD D18] (query delegation)
  is the long-term fix.

- **Conversation ID tracking before RFD 050.** Without `jp conversation new`,
  the plugin must use a heuristic (most recently created conversation, or parse
  stdout) to identify the conversation created by the subprocess. This is
  fragile. RFD 050 is the clean solution.

- **Internal-only scope.** The workflow patterns here (review → refine → accept
  → implement → done) could be useful beyond JP's own development. Keeping the
  plugin internal limits its reach. However, the patterns are encoded in the
  plugin's structure, not hidden — a future generalization is possible without
  redesigning the approach.

- **Terminal ownership during subprocess.** The `jp` subprocess takes over the
  terminal for `$EDITOR` and streaming output. The plugin cannot display
  progress or status while the subprocess runs. This is acceptable for an
  interactive workflow where the contributor is directly engaged.

- **Dedicated tools add surface area.** `rfd_decision`, `rfd_section`, and
  `rfd_explore_summary` are purpose-built tools that need to be implemented
  and maintained. They are thin wrappers around file writes, but they still
  need parameter validation, error handling, and tests. The payoff is
  stronger guarantees and better LLM ergonomics than raw `fs_modify_file`
  with system prompt constraints.

- **Two-stage confirmation is slower.** The intent-then-write pattern for
  `draft` means each section involves two rounds of contributor interaction
  (approve plan, then approve write). For contributors who trust the AI and
  want speed, this is friction. However, the quality of RFD output is
  measurably better when the contributor shapes each section before it is
  written — the friction is the feature.

## Alternatives

### Shell scripts in the justfile

Implement the entire workflow as `just` tasks using shell scripts, similar to
the existing `rfd-*` tasks.

Rejected because:

- The justfile is already 600+ lines. Adding a stateful workflow manager in
  shell increases maintenance burden.
- Shell-based state management (parsing JSON with `jq`, interactive prompts,
  error handling) is fragile and hard to test.
- Building this as a command plugin dogfoods the plugin system, surfacing gaps
  that benefit all plugin authors.

### `just` module with a standalone script

A `just` module (`rfd.just`) that delegates to a standalone bash script for the
workflow logic. Keeps the justfile clean while staying in shell.

Rejected for the same reasons as above, plus: splitting the logic across a
`just` module and a script creates two places to look for RFD workflow logic.

### General-purpose `jp rfd` subcommand

Build the workflow as a first-class `jp` subcommand in `jp_cli`, available to
all users.

Rejected because this workflow is specific to JP's development process. The
opinionated choices (which models to use, what personas to apply, how phases
map to the Implementation Plan section) reflect how *this project* works. A
general tool should be configurable; this plugin is deliberately opinionated.

### Command aliases for ergonomics

Allow `jp rfd` as an alias for `jp dev rfd`. This would require a command alias
feature in JP itself. Deferred as a separate concern — the `dev` subcommand
path works fine, and aliases can be added later without changing the plugin.

## Non-Goals

- **Registry publication.** The plugin is not advertised in the plugin registry.
  It is built and installed locally.
- **Generalization.** The workflow is tailored to JP's RFD process. Making it
  configurable for other projects is future work.
- **Query delegation via protocol.** The plugin spawns `jp` as a subprocess.
  Migrating to [RFD D18]'s protocol-based query delegation is a future
  improvement.
- **Automated review satisfaction tracking.** The plugin does not use AI to
  determine whether review points have been addressed. Dismissal is an explicit
  human action after each refinement step.

## Risks and Open Questions

- **RFD 050 dependency.** The clean conversation ID flow depends on
  `jp conversation new`. Without it, the plugin uses heuristics to identify the
  conversation created by the subprocess. The heuristic (most recently created
  conversation) can fail if another `jp` process creates a conversation
  concurrently. For a single-user development workflow this is unlikely but not
  impossible. Mitigation: implement RFD 050 first, or accept the race condition
  for v1.

- **Plan extraction accuracy.** Extracting phases from the Implementation Plan
  section using structured output depends on the LLM correctly identifying the
  phase boundaries. RFD Implementation Plans are already structured with `###
  Phase N:` headings, so the extraction is mostly mechanical. A regex-based
  fallback could be added if the LLM consistently misidentifies phases.

- **Phase summary quality.** The `implement` step relies on the assistant
  producing a useful summary at the end of the conversation. If the
  conversation is long and complex, the summary may miss important details.
  Mitigation: the system prompt explicitly requests the summary format, and
  the contributor can edit the saved `phases/N.md` file manually.

- **Review attachment cost.** Each active review file is attached to `refine`
  conversations. If reviews are verbose (multi-page), the token cost grows.
  The dismiss-after-refine workflow limits accumulation, but a single review
  from a thorough model can still be large. Mitigation: the contributor can
  edit review files to trim them, or dismiss reviews they consider addressed
  without running `refine`.

- **Explore quality varies by model.** The explore phase uses a cheap model
  (sonnet) for cost efficiency. If the model's research summary is shallow or
  misses relevant code paths, the converge phase starts from a weak
  foundation. Mitigation: the contributor can re-run explore with
  `--model=opus`, or manually edit `explore.md` to add context the model
  missed.

- **Section-by-section drafting assumes template structure.** The `rfd_section`
  tool needs to know which sections exist and where they start/end in the
  file. This works for RFDs created from the standard templates (which have
  `## Section` headers). Free-form RFDs that deviate from the template
  structure may confuse the tool. Mitigation: the tool can fall back to
  appending content at the end of the file, and the contributor can
  rearrange manually.

## Implementation Plan

### Phase 1: Plugin skeleton and status display

Scaffold `crates/internal/dev/` with the plugin protocol boilerplate. Implement
`jp dev rfd <NNN>` to resolve the RFD file, parse its metadata, scan the state
directory, and print the status summary. Include the authoring state (explore,
decisions, sections drafted) in the status display. No LLM interaction.

Depends on: [RFD 072] Phase 1 (plugin protocol core). Can be merged
independently of other phases.

### Phase 2: Dedicated RFD tools

Implement `rfd_decision`, `rfd_section`, and `rfd_explore_summary` as MCP tools
in `.config/jp/tools/src/rfd/`. These are thin wrappers around file writes with
parameter validation and consistent formatting. Register them in
`.jp/mcp/tools/rfd/`.

Can be merged independently of the plugin (the tools are useful even when
invoked manually via `jp query --cfg skill/rfd`).

### Phase 3: Author workflow (explore, converge, draft)

Implement `jp dev rfd author "title"`, `jp dev rfd <NNN> explore`,
`jp dev rfd <NNN> converge`, and `jp dev rfd <NNN> draft`. The `jp.rs` helper
handles subprocess spawning with inherited terminal. Each step configures the
appropriate model, tools, and attachments. Response extraction uses the plugin
protocol's `read_events` for the explore artifact.

The converge step enables `rfd_decision` in ask mode. The draft step enables
`rfd_draft` and `rfd_section` in ask mode and encodes the two-stage
confirmation rule in the system prompt.

Depends on: Phase 1 (skeleton), Phase 2 (tools).

### Phase 4: Review and refine

Implement `jp dev rfd <NNN> review` and `jp dev rfd <NNN> refine`. The
post-refine dismiss prompt is a simple stdin interaction.

Depends on: Phase 1.

### Phase 5: Accept and plan extraction

Implement `jp dev rfd <NNN> accept`. Plan extraction uses `jp query` with
`--schema` and `--format=json` (no terminal needed for this step). Lifecycle
promotion calls `just rfd-promote`.

Depends on: Phase 4 (for the established `jp.rs` helper).

### Phase 6: Implement and done

Implement `jp dev rfd <NNN> implement` and `jp dev rfd <NNN> done`. Phase
summaries are written to `phases/N.md`. The `done` command calls `just
rfd-promote`.

Depends on: Phase 5 (plan must exist for `implement` to know the next phase).

### Phase 7: Implementation-phase review

Adapt `review` and `refine` to work in the Accepted state, attaching
phase summaries and storing reviews in `impl-reviews/`.

Depends on: Phase 6.

## References

- [RFD 001: The JP RFD Process][RFD 001] — the lifecycle this plugin
  automates.
- [RFD 003: JP-Assisted RFD Writing][RFD 003] — the `rfd` skill that provides
  the foundation for AI-assisted RFD work.
- [RFD 050: Scripting Ergonomics][RFD 050] — `jp conversation new` for
  capturing conversation IDs.
- [RFD 072: Command Plugin System][RFD 072] — the plugin protocol this binary
  uses.
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18] —
  future protocol-based query delegation.

[RFD 001]: 001-jp-rfd-process.md
[RFD 003]: 003-jp-assisted-rfds.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
[RFD 072]: 072-command-plugin-system.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
