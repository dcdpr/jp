# RFD 003: JP-Assisted RFD Writing

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Summary**: Adds an `rfd` skill configuration so JP can assist with RFD
  research, structure, and editing as a collaborator.

## Summary

This RFD proposes an `rfd` skill configuration for writing RFDs with JP.
The skill attaches the RFD process document as reference material, enables
read-only tools for codebase exploration, and grants guarded write access so JP
acts as a drafting partner while the contributor remains the author.

## Motivation

Writing an RFD requires following the project's conventions ([RFD 001]), reading
existing code and documentation to inform the design, and structuring a clear
proposal.
JP handles each of these tasks individually; a dedicated skill configuration
combines them into one workflow by loading the process guidelines, granting read
access to the codebase, and instructing the LLM on how to collaborate on RFDs.

Per [RFD 002], contributors write prose that represents their own thinking in
their own words and own what they ship.
Rather than writing the RFD for you, JP helps you write a better one: it points
out gaps in your reasoning, suggests structure, finds relevant code to
reference, and drafts sections for you to rewrite in your own voice.

## Design

### The `rfd` Skill

The skill lives at `.jp/config/skill/rfd.toml` and loads into any conversation:

```sh
jp query --cfg skill/rfd "Help me draft an RFD about X"
```

It also composes with other configs:

```sh
jp query -c personas/architect -c skill/rfd "Review my draft RFD"
```

The skill has four parts:

1. **Attachment**: Attaches [RFD 001] so the LLM has the project's RFD
   conventions, templates, and writing style guidelines in context.

2. **System prompt sections**: Establishes the LLM's role as collaborator rather
   than author, defines the collaboration workflow, and sets quality checks and
   writing style reminders.

3. **Read-only tools**: Runs file-reading, listing, and search tools in
   unattended mode so the LLM can explore the codebase and documentation without
   prompting on each call.

4. **Guarded write access**: Enables `rfd_draft` and `fs_modify_file` in `ask`
   mode so the LLM can create RFD files and propose targeted edits that require
   user confirmation before running.

### Why a Skill

JP loads and merges composable configs via `--cfg` across three categories:

- **Knowledge** configs provide domain rules and conventions without enabling
  tools or attaching files.
- **Skills** add capabilities by enabling tools, attaching reference material,
  and adding focused system prompt sections.
- **Personas** define complete assistant identities (name, model, core prompt)
  and often extend skills and knowledge.

Because RFD assistance combines tools, a reference attachment, and behavioral
instructions, it belongs in `skill/`.
As a skill, it layers onto the active persona ("the architect who can also help
with RFDs") rather than replacing it with a separate "RFD assistant" identity.

### Extended Skills and Tools

The skill extends two existing skills:

- **`read-files.toml`**: Enables `fs_read_file`, `fs_list_files`,
  `fs_grep_files`, and `fs_grep_user_docs` in unattended mode.
- **`project-discourse.toml`**: Enables `github_issues` and `github_pulls` in
  unattended mode.

It also enables two write tools in `ask` mode, which require user confirmation
before execution:

- **`rfd_draft`**: Creates a new RFD from a template (rfd or adr), assigns the
  next available number, and populates the metadata header.
- **`fs_modify_file`**: Proposes targeted edits to existing files.

No other write tools are enabled.

### Attachments

| Attachment                       | Purpose                                |
| -------------------------------- | -------------------------------------- |
| `docs/rfd/001-jp-rfd-process.md` | The RFD process: lifecycle, templates, |
|                                  | sections, writing style, tooling.      |

Only [RFD 001] is attached.
Because the system prompt sections already encode the assistant's behavioral
guidance, attaching this RFD as well would be redundant.

Attaching the full text of [RFD 001] costs tokens, but gives the LLM complete,
accurate reference material.
When the LLM needs other RFDs, it can read them on demand with `fs_read_file`.

### System Prompt Sections

The skill adds four `system_prompt_sections`, all tagged `rfd_skill`:

| Section                        | Purpose                                  |
| ------------------------------ | ---------------------------------------- |
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

Framing the quality checks as questions (such as "Does the Motivation explain
why?") prompts the LLM to evaluate the draft rather than generate filler.

### Typical Usage

**Starting a new RFD from scratch:**

```sh
jp query --new --cfg skill/rfd \
  "I want to write an RFD about adding a plugin registry for Wasm tools. \
   Help me think through the structure."
```

The LLM uses `rfd_draft` to create the initial file from the template and
`fs_modify_file` to fill in sections as the conversation progresses.

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

The LLM proposes an `fs_modify_file` call, and the user reviews the diff before
confirming or rejecting it.

### Relationship to RFD 002

[RFD 002] requires contributors to write prose that represents their thinking in
their own words.
The skill follows that guideline:

- The system prompt frames JP as a **collaborator, not author**: it assists with
  structure, research, and review rather than producing the finished document.
- When JP drafts a section on request, the contributor rewrites it; the draft is
  a starting point, not a submission.
- The quality checks ask evaluative questions ("Does the Motivation explain
  why?") rather than generating prose.

This follows the "LLMs as editors" and "LLMs as researchers" patterns encouraged
in [RFD 002], and avoids the "LLMs as writers" anti-pattern for substantive
prose.

## Alternatives

### Manual Flags Instead of a Dedicated Skill

Contributors could pass `-c skill/read-files` and `-a
docs/rfd/001-jp-rfd-process.md` each time they want help with an RFD.

Rejected because contributors can easily forget the attachment, and the LLM
receives no collaborator guidance or quality check prompts.
A dedicated skill packages these defaults.

### A Persona Instead of a Skill

An `rfd-writer` persona could define a dedicated name, model, and full system
prompt.

Rejected because a persona replaces the active assistant identity, whereas RFD
assistance is a capability added to an existing persona (such as an architect or
developer persona).
A skill composes with any persona.

### A Knowledge Config Instead of a Skill

Knowledge configs provide domain conventions, but cannot enable tools or attach
files.
Because RFD assistance requires research tools, `fs_modify_file` for edits, and
[RFD 001] as an attachment, a knowledge config is insufficient.

## Non-Goals

- **Automated RFD generation**: Producing complete RFDs from a one-line prompt
  would defeat the purpose of writing an RFD.
- **RFD validation or linting**: Automated checks for metadata format and
  required sections are a separate concern left to a future RFD.
- **Attaching all existing RFDs**: Attaching the full RFD corpus would waste
  tokens on irrelevant context.
  Only [RFD 001] is attached; the LLM reads other RFDs on demand with
  `fs_read_file`.

## Risks and Open Questions

- **Token cost of the attachment**: At roughly 4,000 words, [RFD 001] consumes a
  significant share of the context window.
  Attaching the full document is preferred for accuracy, but its core
  conventions can be summarized into a system prompt section if token cost
  becomes a problem.
- **Skill composability**: The `extends` mechanism assumes extended skills do
  not conflict with each other or with the active persona, which may break if
  tool configurations diverge.
- **Attaching RFD 002**: [RFD 002] contains relevant guidance on LLM use, but is
  omitted to save tokens because the system prompt already captures its core
  principle (collaborator, not author).
  It can be attached later if needed.

## Implementation Plan

1. Create `.jp/config/skill/rfd.toml` with the configuration described above.
2. Test the skill in real RFD drafting sessions to verify that the LLM follows
   [RFD 001] conventions, uses read-only tools for research, and stays in the
   collaborator role.
3. Adjust the prompt guidance based on drafting experience.

## References

- [RFD 001: The JP RFD Process][RFD 001]: the RFD process this skill teaches JP
  to follow.
- [RFD 002: Using LLMs in the JP Project][RFD 002]: the LLM usage guidelines
  that define this skill's boundaries.
- Skill: Read Files (`.jp/config/skill/read-files.toml`): extended for codebase
  exploration.
- Skill: Project Discourse (`.jp/config/skill/project-discourse.toml`): extended
  for GitHub issue and pull request context.

[RFD 001]: 001-jp-rfd-process.md
[RFD 002]: 002-using-llms.md
