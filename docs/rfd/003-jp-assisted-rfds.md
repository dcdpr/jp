# RFD 003: JP-Assisted RFD Writing

- **Status**: Draft
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17

## Summary

This RFD describes how contributors can use JP itself to help write RFDs for the
JP project. It proposes an `rfd` skill configuration that teaches JP the RFD
process, attaches the process document as reference material, enables read-only
tools for codebase exploration, and provides guarded write access for applying
edits — so that JP can serve as a drafting partner without displacing the
contributor's own thinking.

## Motivation

Writing an RFD involves understanding the project's conventions ([RFD 001][]),
reading existing code and documentation to inform the design, and structuring a
proposal that is clear and well-reasoned. JP is already good at all of these
tasks individually. A dedicated skill configuration can combine them into a
single workflow: load the process guidelines, give the LLM read access to the
codebase, and provide system prompt guidance on how to be a useful RFD
collaborator.

The goal is not to have JP write the RFD for you. [RFD 002][] is clear on this
point: prose that represents your thinking should be written in your own words,
and you own what you ship. The goal is to have JP help you write a better RFD —
by pointing out gaps in your reasoning, suggesting structure, finding relevant
code to reference, and drafting sections you can then rewrite in your voice.

## Design

### The `rfd` Skill

A new skill configuration at `.jp/config/skill/rfd.toml` that can be loaded into
any conversation:

```sh
jp query --cfg skill/rfd "Help me draft an RFD about X"
```

Or combined with other configs:

```sh
jp query -c personas/architect -c skill/rfd "Review my draft RFD"
```

The skill consists of four parts:

1. **Attachment** — RFD 001 (the process document) is loaded as a file
   attachment, giving the LLM direct access to the project's RFD conventions,
   templates, and writing style guidelines.

2. **System prompt sections** — focused prompt sections that establish the LLM's
   role as an RFD collaborator (not author), describe the collaboration
   workflow, and set quality checks and writing style reminders.

3. **Read-only tools** — file reading, listing, and searching tools run in
   unattended mode, so the LLM can explore the codebase and existing
   documentation without prompting the user for each call.

4. **Guarded write access** — `fs_modify_file` and `rfd_draft` are enabled in
   `ask` mode, so the LLM can create new RFD files and propose targeted edits,
   but the user must confirm each action before it is applied.

### Why a Skill

JP configs are composable — personas, skills, and knowledge configs can all be
loaded and merged via `--cfg`. The question is which directory this config
belongs in.

- **Knowledge** configs provide domain expertise (rules, conventions) but don't
  enable tools or attach files.
- **Skills** add capabilities: they enable tools, attach reference material, and
  add focused system prompt sections.
- **Personas** define complete assistant identities (name, model, core prompt).
  They often extend skills and knowledge.

This config enables tools (read-only via extension, `fs_modify_file` in ask
mode), attaches a reference document, and adds behavioral instructions. That's a
capability — a skill. It layers on top of whatever persona is already active:
"the architect who can also help with RFDs," not "the RFD assistant."

### What the Skill Extends

The skill extends two existing skills:

- **`read-files.toml`** — enables `fs_read_file`, `fs_list_files`,
  `fs_grep_files`, and `fs_grep_user_docs` in unattended mode.
- **`project-discourse.toml`** — enables `github_issues` and `github_pulls` in
  unattended mode.

Additionally, the skill enables two tools in `ask` mode:

- **`rfd_draft`** — creates a new RFD from the appropriate template (rfd or
  adr), assigning the next available number and filling in metadata.
- **`fs_modify_file`** — proposes edits to existing files.

Both require user confirmation before execution. No other write tools are
enabled.

### What Gets Attached

| Attachment                       | Purpose                                  |
|----------------------------------|------------------------------------------|
| `docs/rfd/001-jp-rfd-process.md` | The RFD process: lifecycle, templates,   |
|                                  | sections, writing style, tooling.        |

Only RFD 001 is attached. The skill's own behavioral guidance is encoded in the
system prompt sections, so attaching the skill's source RFD (this document)
would be redundant — the system prompt already contains everything the assistant
needs to know about how to behave.

Loading the full content of RFD 001 as an attachment costs tokens but ensures
the LLM has complete, accurate reference material for writing RFDs. The LLM can
use `fs_read_file` to look up other specific RFDs when needed.

### System Prompt Sections

The skill adds four `system_prompt_sections`, all tagged `rfd_skill`:

| Section                        | Purpose                                  |
|--------------------------------|------------------------------------------|
| **Skill: RFD Writing**         | Establishes the collaborator-not-author  |
|                                | role. Lists what the LLM should and      |
|                                | should not do.                           |
| **RFD Collaboration Workflow** | Six-step workflow: understand intent,    |
|                                | research context, suggest structure,     |
|                                | review drafts, draft on request, apply   |
|                                | edits.                                   |
| **RFD Quality Checks**         | Questions to evaluate a draft against    |
|                                | (Summary clear? Motivation explains why? |
|                                | Alternatives explored?).                 |
| **RFD Writing Style**          | The writing conventions from RFD 001 in  |
|                                | condensed form.                          |

The quality checks are framed as questions, not generators. "Does the Motivation
explain why?" prompts the LLM to evaluate, not produce filler.

### Typical Usage

**Starting a new RFD from scratch:**

```sh
jp query --new --cfg skill/rfd \
  "I want to write an RFD about adding a plugin registry for Wasm tools. \
   Help me think through the structure."
```

The LLM can use the `rfd_draft` tool to create the initial file from the
appropriate template, then use `fs_modify_file` to fill in sections as the
conversation progresses.

**Getting feedback on a draft:**

```sh
jp query --new --cfg skill/rfd \
  --attach docs/rfd/004-wasm-plugin-registry.md \
  "Review this draft RFD. What's missing? What's unclear?"
```

**Researching context for a section:**

```sh
jp query --cfg skill/rfd \
  "I'm writing the Alternatives section for my Wasm registry RFD. \
   What approaches do other plugin systems use? Check our existing \
   Wasm architecture doc for relevant context."
```

**Applying edits to a draft:**

```sh
jp query --cfg skill/rfd \
  -a docs/rfd/004-wasm-plugin-registry.md \
  "The Motivation section is weak. Rewrite it to emphasize the \
   security benefits of sandboxed plugins."
```

The LLM will propose an `fs_modify_file` call. The user reviews the diff and
confirms or rejects it.

### Relationship to RFD 002

[RFD 002][] establishes that LLM-generated prose that represents your thinking
should generally be written in your own words. This skill is designed to work
within that guideline:

- The system prompt explicitly frames JP as a **collaborator, not author**. It
  assists with structure, research, and review — it does not produce the
  finished document.
- When JP drafts text on request, the expectation is that the contributor
  rewrites it. The draft is a starting point, not a submission.
- The quality check sections focus on asking questions ("Does the Motivation
  explain why?") rather than generating answers.

This matches the "LLMs as editors" and "LLMs as researchers" patterns from RFD
002, which are encouraged. It deliberately avoids the "LLMs as writers" pattern
for substantive prose.

## Alternatives

### No dedicated skill — use read-files and attach RFD 001 manually

Contributors could load `-c skill/read-files` and `-a
docs/rfd/001-jp-rfd-process.md` each time they want help with an RFD.

Rejected because: it is easy to forget the attachment, the LLM gets no
behavioral guidance about its role as collaborator, and there are no quality
check prompts. The skill encodes the right defaults.

### A persona instead of a skill

We could create an `rfd-writer` persona with a dedicated name, model, and full
system prompt.

A persona would work, but a skill is a better fit. A persona sets an assistant
identity — the persona *is* the assistant. The RFD capability is something you
add *to* an assistant. You want "the architect who can also help with RFDs" or
"the dev who can also help with RFDs," not a separate RFD-only identity. A skill
composes naturally with any persona.

### A knowledge config instead of a skill

Knowledge configs provide domain expertise but don't enable tools or attach
files. The RFD skill needs both (read-only tools for research, `fs_modify_file`
for applying edits, and RFD 001 as an attachment), so knowledge alone is
insufficient.

## Non-Goals

- **Automated RFD generation**: This skill does not aim to produce complete RFDs
  from a one-line prompt. That would undermine the purpose of writing one.
- **RFD validation or linting**: Automated checks for metadata format, section
  presence, etc. are a separate concern and could be a future RFD.
- **Loading all existing RFDs as context**: Only RFD 001 is attached. Loading
  the full RFD corpus would be token-expensive and mostly irrelevant. The LLM
  can use `fs_read_file` to look up specific RFDs when needed.

## Risks and Open Questions

- **Token cost of the attachment**: RFD 001 is ~4000 words. This consumes a
  meaningful chunk of the context window. If this becomes a problem, the key
  conventions could be summarized into a system prompt section instead. For now,
  full document attachment is preferred because it ensures accuracy.
- **Skill composability**: The `extends` mechanism assumes extended skills don't
  conflict with each other or with the active persona. This is true today but
  could become an issue if tool configurations diverge.
- **Should RFD 002 also be attached?** It contains relevant guidance on LLM use.
  Omitted for now to reduce token cost — the system prompt captures the key
  principle (collaborator, not author). Can be added later if needed.

## Implementation Plan

1. Create `.jp/config/skill/rfd.toml` with the configuration described above.
2. Test with a few real RFD drafting sessions to validate that the system prompt
   sections, tools, and attachment work as expected. Verify that the LLM
   references RFD 001 conventions correctly, uses tools to research context, and
   respects the collaborator role.
3. Adjust guidance based on experience.

## References

- [RFD 001: The JP RFD Process](001-jp-rfd-process.md) — the process this skill
  teaches JP to follow.
- [RFD 002: Using LLMs in the JP Project](002-using-llms.md) — the guidelines on
  LLM use that inform this skill's boundaries.
- [Skill: Read Files](../../.jp/config/skill/read-files.toml) —
  extended by this skill for codebase exploration.
- [Skill: Project Discourse](../../.jp/config/skill/project-discourse.toml)
  — extended by this skill for GitHub context.

[RFD 001]: 001-jp-rfd-process.md
[RFD 002]: 002-using-llms.md
