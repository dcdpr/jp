# RFD D05: Internal Dev Plugin for RFD Workflows

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Requires**: [RFD 050]
- **Summary**: Orchestrates AI-assisted RFD exploration, drafting, review, and
  phased implementation through an internal `jp-dev` command plugin.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Its plugin was split by concern into later RFDs, and the remaining workflow
> needs a redesign for the current plugin protocol.
>
> The single internal `jp dev` plugin became a family of plugins: [RFD 116]
> moves the RFD lifecycle commands (which this draft delegated to `just
> rfd-promote`) into `jp rfd`, and [RFD 121] and [RFD 122] move release
> management into `jp release`.
> The AI-assisted steps between lifecycle transitions (explore, converge, draft,
> review, refine, implement) were not carried forward; a future RFD will build
> them on top of `jp rfd`, using the host's `query` request instead of spawning
> `jp query` subprocesses.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces `jp-dev`, an internal command plugin that orchestrates
AI-assisted workflows across the RFD lifecycle: authoring new RFDs through a
phased explore → converge → draft process, reviewing and refining drafts, and
executing implementation plans phase by phase.
The plugin manages a state directory alongside the RFD documents, spawns `jp`
conversations with step-specific models and tools, and carries artifacts
(research summaries, locked decisions, reviews, and phase summaries) from one
step to the next.

## Motivation

The existing RFD tooling (`just rfd-*`) handles lifecycle transitions: creating
drafts, promoting status, superseding, and abandoning.
It does not handle the work between transitions: exploring a problem space,
converging on design decisions, drafting the document section by section,
reviewing and refining the draft, and executing the implementation plan phase by
phase.

Today this work happens through ad-hoc `jp query` invocations where the
contributor manually assembles the persona, model, attachments, and prompt for
each step.
Context does not carry across sessions: research stays in the conversation where
it was gathered, brainstorming decisions remain buried in chat history, reviews
are not saved as artifacts, and implementation summaries from one phase do not
feed into the next.
There is also no way to inspect where an RFD stands in its workflow.

The authoring gap is especially costly.
Complex RFDs benefit from a phased process: fast, cheap models for initial
research (reading code, scanning prior art, searching crates); strong models for
design convergence (evaluating trade-offs, locking decisions); and
section-by-section drafting with explicit review gates.
Today, contributors do all of this in a single conversation with one model and
tool set, or across several disconnected conversations that lose context between
sessions.

The `dev` plugin automates this orchestration.
It is internal to the JP project and is not intended for general distribution.
Each workflow step is a subcommand that:

1. Selects the model and tool set for the task.
2. Attaches artifacts from prior steps as context.
3. Opens `$EDITOR` so the contributor can steer the prompt.
4. Saves the output as a durable artifact.
5. Feeds that artifact into subsequent steps.

## Design

### User Experience

The entry point is `jp dev rfd`.
The status display adapts to the RFD's current state in the workflow:

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

The plugin accepts either bare numbers (`045`, `45`) or draft IDs (`D32`).
It resolves the RFD file by globbing `docs/rfd/045-*.md` or `docs/rfd/D32-*.md`.

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

The directory is gitignored.
All state is derived from filesystem presence:

| State               | Source                                   |
| ------------------- | ---------------------------------------- |
| RFD status          | Parsed from the `- **Status**: ...` line |
| Explore complete    | `explore.md` file presence               |
| Decisions exist     | `decisions.md` file presence             |
| Plan exists         | `plan.json` file presence                |
| Phase count         | `plan.json` array length                 |
| Completed phases    | Which `phases/N.md` files exist          |
| Active reviews      | Files in `reviews/`                      |
| Active impl reviews | Files in `impl-reviews/`                 |

There is no `state.json`; the filesystem is the state, so recovering from a
broken state only requires adding or deleting a file.

### Workflow Steps

Each step spawns a `jp` subprocess that inherits the terminal, giving the
contributor `$EDITOR` for composing the prompt, streaming Markdown rendering,
and tool call output.
After the subprocess exits, the plugin fetches the conversation's last
`chat_response` event over the plugin protocol and saves its message as a
Markdown file (see [Response Extraction](#response-extraction)).

#### Creating conversations

Each workflow step runs in its own conversation so the contributor's active
conversation stays unchanged.
The plugin runs `jp conversation new` ([RFD 050]) to create a conversation and
capture its ID, then runs `jp query --id=<ID>` against it.

Before [RFD 050] is implemented, the plugin falls back to parsing the
conversation ID from `jp query --new` output or selecting the most recently
created conversation in the workspace after the subprocess exits.

### Author Workflow

The author workflow covers the three steps before the RFD document is complete:
`explore`, `converge`, and `draft`.
Each step uses a distinct model and tool profile and produces an artifact that
feeds into the next step of the full lifecycle:

```
explore → converge → draft → review → refine → accept → implement → done
```

#### `explore`

Research the problem space with a fast, cheap model.

```
jp dev rfd author "Better tracing for JP"
jp dev rfd D32 explore                    # re-run after draft exists
jp dev rfd D32 explore --model=flash       # override model
```

- **Model**: sonnet (fast and cheap for research and brainstorming).
- **Tools**: read-only (`fs_read_file`, `fs_grep_files`, `fs_list_files`,
  `github_issues`, `github_pulls`), web access (`web_fetch`), and crate research
  (`crates_search`, `crate_readme`, `crate_search_items`).
  No general write tools.
- **Attaches**: the contributor's problem statement or notes.
- **Behavior**: freeform conversation in which the assistant investigates the
  codebase, reads related RFDs, searches for crates and prior art, and asks
  clarifying questions.
- **Artifact**: `explore.md`, written from the assistant's final response when
  the conversation ends.

Running `jp dev rfd author "title"` creates a new state directory, records the
title, and starts `explore` immediately.
Running `jp dev rfd D32 explore` re-runs research for an existing draft;
contributors can run `explore` multiple times to deepen research, and each run
overwrites `explore.md`.

#### `converge`

Converge on design decisions with a strong model.

```
jp dev rfd D32 converge
```

- **Model**: opus (for evaluating trade-offs and making design decisions).
- **Tools**: the `explore` read-only tools plus `rfd_decision` (see [Dedicated
  RFD Tools](#dedicated-rfd-tools)).
  No general file write access.
- **Attaches**: `explore.md`, the original problem statement, and [RFD 001].
- **Behavior**: the assistant proposes options and iterates with the
  contributor, calling `rfd_decision` to add decisions, update existing ones, or
  mark them as rejected.
- **Artifact**: `decisions.md`, updated incrementally through `rfd_decision`
  calls during the conversation.

Because `rfd_decision` runs in `ask` mode, the contributor confirms each write
and can reject or revise a decision before it lands in `decisions.md`.

The conversation continues until the contributor exits.
The state machine requires `decisions.md` to exist before `draft` can run, and
the contributor can re-enter `converge` at any time to revise decisions, even
after drafting has started.

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

Draft the RFD document section by section with explicit review gates.

```
jp dev rfd D32 draft
```

- **Model**: opus.

- **Tools**: read-only tools, plus `rfd_draft` (to create the file) and
  `rfd_section` (to write individual sections).
  See [Dedicated RFD Tools](#dedicated-rfd-tools).
  No general file write access.

- **Attaches**: `decisions.md`, `explore.md`, and [RFD 001].

- **Behavior**: the assistant follows a two-stage confirmation process for each
  section:

  1. **State intent.** The assistant names the next section, outlines its plan
     (what points to cover, how to frame them, and what to exclude), and waits
     for approval.
  2. **Contributor reviews intent.** The contributor approves, pushes back, or
     redirects the plan.
  3. **Write the section.** The assistant calls `rfd_section` in `ask` mode.
     The contributor compares the written section against the agreed plan and
     confirms or rejects the write.
  4. **Proceed to next section.** The assistant waits for confirmation before
     moving on.

- **Artifact**: the RFD file, created by `rfd_draft` and populated by
  `rfd_section`.

The system prompt encodes the two-stage confirmation rule:

> Before writing each section: (1) state which section you will write next, (2)
> describe your plan (what points you will cover, how you will frame them, and
> what you are deliberately excluding), (3) wait for approval before writing,
> (4) do not proceed to the next section until the contributor confirms.

To revise a section within the same conversation, the assistant calls
`rfd_section` again for that section, overwriting its previous content.

### Dedicated RFD Tools

The author workflow uses purpose-built tools instead of raw `fs_modify_file`.
These tools expose domain operations ("decisions" and "sections") rather than
raw file edits and diffs.

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

Writes to `docs/rfd/.state/<id>/decisions.md` in `ask` mode.
The tool formats the file directly so the LLM never edits raw Markdown.

#### `rfd_section`

Writes a single section of the RFD file during the `draft` phase.

```
Parameters:
  section:  string    (e.g., "summary", "motivation", "design")
  content:  string    (the markdown content for this section)
```

Resolves the RFD file path from workflow state and calls `fs_modify_file` in
`ask` mode, replacing the template placeholder or existing content for that
section.

#### `rfd_explore_summary`

Writes the research summary for the `explore` phase.

```
Parameters:
  content:  string    (the research summary markdown)
```

Writes to `docs/rfd/.state/<id>/explore.md` in `ask` mode.

These tools live alongside `rfd_draft`, `rfd_promote`, and the other RFD tools
in `.jp/mcp/tools/rfd/` and `.config/jp/tools/src/rfd/`.

### Model and Tool Selection

Each workflow step uses a model and tool set appropriate to its purpose:

| Step        | Default Model | Tools                         | Write Access                |
| ----------- | ------------- | ----------------------------- | --------------------------- |
| `explore`   | sonnet        | read-only, web, crate search, | `rfd_explore_summary` (ask) |
|             |               | `rfd_explore_summary`         |                             |
| `converge`  | opus          | read-only, `rfd_decision`     | `rfd_decision` (ask)        |
| `draft`     | opus          | read-only, `rfd_draft`,       | `rfd_draft` (ask),          |
|             |               | `rfd_section`                 | `rfd_section` (ask)         |
| `review`    | gemini-pro    | read-only                     | none                        |
| `refine`    | opus          | read-only, `rfd_section`      | `rfd_section` (ask)         |
| `implement` | opus          | full dev toolset              | yes                         |

All steps accept `--model=<id>` to override the default.

#### `review`

Spawns a `jp` conversation with the architect persona and a review-focused model
(default: Gemini Pro), attaches the RFD document, and opens `$EDITOR` with a
pre-filled review prompt that the contributor can edit before sending.
When the conversation ends, the assistant's final response is saved to
`reviews/<timestamp>.md`.
Multiple reviews can coexist as independent conversations.

```
jp dev rfd 045 review                  # default model
jp dev rfd 045 review --model=opus     # override model
```

#### `refine`

Spawns a conversation with the dev persona (default: Opus), attaches the RFD
document and all files in `reviews/`, and opens `$EDITOR` so the contributor can
steer the refinement.
When the conversation ends, the plugin lists active reviews and prompts to
dismiss resolved ones:

```
Active reviews:
  1. reviews/1721500000.md - "Error handling in Design section needs..."
  2. reviews/1721586000.md - "Migration path is underspecified..."

Dismiss resolved reviews? [numbers, 'all', or enter to skip]: 1
Dismissed reviews/1721500000.md
```

Dismissed reviews are deleted from `reviews/` so they are not attached to future
refinement steps.

#### `accept`

Runs two operations in sequence:

1. **Plan extraction.** Spawns a `jp` conversation with a JSON schema to extract
   the phases defined in the RFD's Implementation Plan section into `plan.json`:

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

2. **Lifecycle promotion.** Calls `just rfd-promote <NNN>` to transition the RFD
   from Discussion to Accepted, creating the GitHub tracking issue and updating
   the metadata header.

If plan extraction fails, the promotion does not run; the contributor can fix
the Implementation Plan section and retry.
If the RFD is still in Draft status, `accept` calls `just rfd-promote` twice:
first to Discussion (assigning a permanent number), and then to Accepted.

#### `implement`

Finds the next unfinished phase from `plan.json` and the existing `phases/N.md`
files, then spawns a conversation with the dev persona (default: Opus) and
attaches:

- The RFD document
- The phase description from `plan.json`
- All previous `phases/N.md` summaries

The system prompt instructs the assistant to implement the phase and end with a
summary of what was built, any deviations from the RFD, and notes for later
phases.
When the conversation ends, the assistant's final response is saved to
`phases/N.md`.

If the first pass is incomplete, the contributor can continue the conversation
with `jp q`; `phases/N.md` is written when the plugin-managed conversation
exits.

#### `done`

Calls `just rfd-promote <NNN>` to transition the RFD from Accepted to
Implemented without any LLM interaction.

### Response Extraction

After a `jp query` subprocess exits, the plugin fetches the assistant's last
message over the plugin protocol:

1. Track the conversation ID obtained from `jp conversation new` (or parsed from
   `jp query` output).
2. Send a `read_events` request for that conversation ID over the host protocol
   connection.
3. Receive the conversation's events as JSON.
4. Scan backward for the last `chat_response` event and extract its `message`
   field.

Using `read_events` decouples the plugin from the on-disk conversation format.

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

The binary is named `jp-dev` and registers the command path `["dev"]`.
It is not published to the plugin registry.

### Subprocess Spawning (`jp.rs`)

The `jp.rs` module spawns `jp` as a subprocess with inherited
stdin/stdout/stderr, passing the configured persona, model, attachments, and
flags.
Its entry point takes a conversation specification (persona, model, attachments,
prompt) and returns the conversation ID after execution, isolating the call site
so subprocess spawning can later be replaced with protocol-based query
delegation ([RFD D18]).

### Review and Refinement During Implementation

The `review` and `refine` commands adapt their behavior based on the RFD's
current status:

| Status     | `review` target    | `refine` target    | Review storage  |
| ---------- | ------------------ | ------------------ | --------------- |
| Discussion | The RFD document   | The RFD document   | `reviews/`      |
| Accepted   | The implementation | The implementation | `impl-reviews/` |

During Discussion, reviews evaluate the *design*.
During Accepted, reviews evaluate the *implementation*: the plugin attaches
completed phase summaries alongside the RFD so the reviewer can check whether
the implementation matches the design.

### Installation

The plugin is built and installed locally via `just`:

```sh
just plugin-build-local
```

This builds all command plugin binaries (including `jp-dev`) and copies them to
the local plugin directory.
Because `crates/internal/*` is already in the Cargo workspace members list, no
workspace configuration changes are needed.

## Drawbacks

- **Subprocess spawning.** Spawning `jp` as a child process for each step runs a
  second `jp` instance that repeats workspace discovery, config loading, and LLM
  setup independently of the host process.
  [RFD D18] (query delegation) is the long-term fix.

- **Conversation ID tracking before RFD 050.** Without `jp conversation new`
  ([RFD 050]), the plugin must parse stdout or select the most recently created
  conversation to identify the subprocess's conversation, which is fragile.

- **Internal-only scope.** Keeping the plugin internal limits a workflow pattern
  (`review → refine → accept → implement → done`) that could apply beyond
  JP's own development, though the structure can be generalized later without
  redesigning the approach.

- **Terminal ownership during subprocess execution.** Because the `jp`
  subprocess inherits the terminal for `$EDITOR` and streaming output, the
  plugin cannot render its own progress or status while a step runs.
  This is acceptable for an interactive workflow where the contributor is
  directly engaged in the terminal.

- **Dedicated tools add surface area.** `rfd_decision`, `rfd_section`, and
  `rfd_explore_summary` are thin wrappers around file writes, but they still
  require parameter validation, error handling, and tests.
  In return, they provide stronger format guarantees and simpler LLM calls than
  raw `fs_modify_file` guided only by system prompt constraints.

- **Two-stage confirmation is slower.** Requiring plan approval before section
  approval in `draft` takes two rounds of contributor interaction per section.
  That extra round slows down contributors who want a quick draft, but steering
  each section before it is written produces higher-quality RFDs.

## Alternatives

### Shell scripts in the justfile

Implement the workflow as `just` tasks using shell scripts, similar to the
existing `rfd-*` tasks.

Rejected because:

- The justfile is already over 600 lines, and adding a stateful workflow manager
  in shell increases maintenance burden.
- Shell-based state management (parsing JSON with `jq`, interactive prompts,
  error handling) is fragile and hard to test.
- Building this as a command plugin dogfoods the plugin system, surfacing gaps
  that benefit all plugin authors.

### `just` module with a standalone script

Delegate from a `just` module (`rfd.just`) to a standalone bash script to keep
the main justfile small.

Rejected for the same reasons as shell scripts in the justfile, and because
splitting the workflow across `rfd.just` and a bash script scatters the logic
across two files.

### General-purpose `jp rfd` subcommand

Build the workflow as a built-in `jp` subcommand in `jp_cli`, available to all
users.

Rejected because the workflow's choices (models, personas, and how phases map to
the Implementation Plan section) are specific to how JP itself is developed
rather than configurable for general use.

### Command aliases for ergonomics

Allow `jp rfd` as an alias for `jp dev rfd`.
Deferred because command aliases would require a new feature in JP itself, and
`jp dev rfd` works without plugin changes until aliases exist.

## Non-Goals

- **Registry publication.** The plugin is built and installed locally, not
  published to the plugin registry.
- **Generalization.** Making the workflow configurable for other projects is
  future work.
- **Query delegation via protocol.** The plugin spawns `jp` as a subprocess;
  migrating to [RFD D18]'s protocol-based query delegation is future work.
- **Automated review satisfaction tracking.** The plugin does not use an LLM to
  judge whether review points have been addressed; the contributor dismisses
  reviews explicitly after each refinement step.

## Risks and Open Questions

- **RFD 050 dependency.** Without `jp conversation new`, the fallback heuristic
  (selecting the most recently created conversation) can pick the wrong
  conversation if another `jp` process starts one concurrently.
  Mitigation: implement [RFD 050] first, or accept the race condition in v1.

- **Plan extraction accuracy.** Extracting phases from the Implementation Plan
  section with structured output depends on the LLM identifying phase boundaries
  accurately.
  Because RFD Implementation Plans use `### Phase N:` headings, extraction is
  mostly mechanical, and a regex fallback can be added if needed.

- **Phase summary quality.** The `implement` step relies on the assistant
  producing a complete summary at the end of the conversation, which may omit
  details after a long session.
  Mitigation: the system prompt specifies the summary format, and the
  contributor can edit `phases/N.md` directly.

- **Review attachment cost.** Attaching every active review file to `refine`
  conversations increases token cost when reviews are long.
  Mitigation: dismissing addressed reviews after each `refine` run bounds
  accumulation, and contributors can trim or delete review files manually.

- **Explore quality varies by model.** The `explore` step uses a cheap model
  (sonnet) by default; if its summary is shallow or misses code paths,
  `converge` starts with incomplete context.
  Mitigation: re-run `explore` with `--model=opus`, or edit `explore.md`
  manually.

- **Section-by-section drafting assumes template structure.** The `rfd_section`
  tool locates sections by the `## Section` headings from the standard RFD
  templates, which may not match free-form RFDs.
  Mitigation: fall back to appending new sections at the end of the file for the
  contributor to reorder.

## Implementation Plan

### Phase 1: Plugin skeleton and status display

Scaffold `crates/internal/dev/` with the plugin protocol boilerplate.
Implement `jp dev rfd <NNN>` to resolve the RFD file, parse its metadata, scan
the state directory, and print the status summary (including authoring state for
explore, decisions, and drafted sections).
No LLM interaction.

Depends on: [RFD 072] Phase 1 (plugin protocol core).
Can be merged independently of other phases.

### Phase 2: Dedicated RFD tools

Implement `rfd_decision`, `rfd_section`, and `rfd_explore_summary` as MCP tools
in `.config/jp/tools/src/rfd/` and register them in `.jp/mcp/tools/rfd/`.

Can be merged independently of the plugin (the tools can also be invoked
manually via `jp query --cfg skill/rfd`).

### Phase 3: Author workflow (explore, converge, draft)

Implement `jp dev rfd author "title"`, `jp dev rfd <NNN> explore`, `jp dev rfd
<NNN> converge`, and `jp dev rfd <NNN> draft`.
Use `jp.rs` to spawn `jp` subprocesses with an inherited terminal, configure the
step-specific model, tools, and attachments, and extract `explore.md` via
`read_events`.
Enable `rfd_decision` in `ask` mode for `converge`, and enable `rfd_draft` and
`rfd_section` in `ask` mode with the two-stage confirmation prompt for `draft`.

Depends on: Phase 1 (skeleton), Phase 2 (tools).

### Phase 4: Review and refine

Implement `jp dev rfd <NNN> review` and `jp dev rfd <NNN> refine`, including the
stdin prompt to dismiss resolved reviews after refinement.

Depends on: Phase 1.

### Phase 5: Accept and plan extraction

Implement `jp dev rfd <NNN> accept`: run `jp query` with `--schema` and
`--format=json` to extract `plan.json`, then call `just rfd-promote`.

Depends on: Phase 4 (for the `jp.rs` helper).

### Phase 6: Implement and done

Implement `jp dev rfd <NNN> implement` (writing phase summaries to
`phases/N.md`) and `jp dev rfd <NNN> done` (calling `just rfd-promote`).

Depends on: Phase 5 (`plan.json` must exist for `implement` to select the next
phase).

### Phase 7: Implementation-phase review

Extend `review` and `refine` to handle RFDs in the Accepted state by attaching
completed phase summaries and storing reviews under `impl-reviews/`.

Depends on: Phase 6.

## References

- [RFD 001: The JP RFD Process][RFD 001]: the lifecycle this plugin automates.
- [RFD 003: JP-Assisted RFD Writing][RFD 003]: the `rfd` skill that provides the
  foundation for AI-assisted RFD work.
- [RFD 050: Scripting Ergonomics][RFD 050]: `jp conversation new` for capturing
  conversation IDs.
- [RFD 072: Command Plugin System][RFD 072]: the plugin protocol this binary
  uses.
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18]: future
  protocol-based query delegation.

[RFD 001]: ../001-jp-rfd-process.md
[RFD 003]: ../003-jp-assisted-rfd-writing.md
[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 116]: ../116-shareable-rfd-ticket-and-site-tooling.md
[RFD 121]: ../121-changelog-fragments-and-validation.md
[RFD 122]: ../122-release-pipeline.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
