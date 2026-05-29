# Ubiquitous Language

JP's domain vocabulary.
Every term here is the project's *agreed name* for a concept that appears in
code, tests, documentation, commits, RFDs, CLI help, and error messages.
Contributors — human and AI — use these terms as written, in every surface
where they appear.

The glossary is split into **clusters**: small groups of closely-related terms
that only make sense together.
When looking up a term, read its cluster — a `Turn` without its surrounding
`Conversation`/`Event`/`Thread` vocabulary is hard to reason about in isolation.
This is the same idea as a bounded context in DDD, scoped to JP's actual
subdomains.

In disagreements between code and this document, the code is authoritative.

> [!NOTE]
> This is the new structure.
> Migration from the [legacy single-page glossary] is in progress; terms not yet
> pulled into a cluster still live there.
> Once every cluster is populated, the legacy page will be removed and inbound
> links updated.

## How to use this glossary

- **Use the exact term.** When the code calls something a `Turn`, don't silently
  start calling it a "message" in a comment, a "round" in documentation, or an
  "exchange" in a commit.
  Paraphrasing accumulates into real confusion.
  Each entry has an **Avoid** section listing the near-synonyms to not reach
  for.

- **Names are contracts.** Renaming a type, field, or concept propagates through
  code, serialized formats, tests, docs, CLI output, and user scripts.
  Don't rename in passing — rename as its own deliberate change, and do it
  thoroughly.

- **Introduce new terms explicitly.** When a new concept emerges, name it.
  Don't leave it as "that thing we do after a tool call."
  Add it to the appropriate cluster and use it consistently from that point on.
  If no cluster fits, add a new one.

- **User-facing and internal names can differ, but drift is a warning.**
  Public-surface terms (CLI flags, config keys, error messages, help text) must
  match user expectations.
  Internal type names can be different.
  When the two diverge without reason, the model is fuzzy, not features-rich.

- **The language evolves.** As understanding sharpens, the names should too.
  Contradictions — "we call it X here but actually it's a Y" — are feedback
  that the model needs work, not to be papered over with aliases.
  Update entries when you resolve a contradiction.

## Clusters

- [**Conversation**] — `Conversation`, `Turn`, `Event`, `Tool Call`, `Inquiry`,
  `Thread`.
  The user-facing notion of "talking to the assistant" and the event log that
  backs it.

Planned clusters (not yet written): *Workspace & Storage*, *Assistant &
Configuration*, *LLM*, *Attachments*, *Tools & Plugins*, *Process*.

## Alphabetical index

A quick lookup for any defined term.
Each entry links to the cluster where it lives.

- **Turn** → [Conversation › Turn]

[**Conversation**]: ./conversation.md
[Conversation › Turn]: ./conversation.md#turn
[legacy single-page glossary]: ../ubiquitous-language.md
