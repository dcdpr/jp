# Architecture

- **Status**: Reference
- **Authors**: jp
- **Date**: 2026-10-31
- **Summary**: Where to start reading about how JP is built: the query pipeline,
  structured output, turn numbering, and the designs not yet built.

JP is a Cargo workspace of small crates, each owning one concern: `jp_cli`
parses commands and drives a query, `jp_llm` talks to providers,
`jp_conversation` holds the event stream, and `jp_storage` and `jp_workspace`
persist it.
The pages below describe how those pieces meet.

Read the [Query Stream Pipeline Architecture] first: it is how a `jp query` turn
runs today, from the streamed response to tool calls and interrupts.
[Structured Output Architecture] and [Indexing and Counting Conventions] build
on it.
[JP Query Architecture] is the March 2026 analysis the pipeline was designed to
answer, kept as a Snapshot.
The knowledge base and Wasm tools pages are designs that have not been built.

<WikiChildren />

[Indexing and Counting Conventions]: 00dxyba-indexing-and-counting-conventions.md
[JP Query Architecture]: 00dxyb5-jp-query-architecture.md
[Query Stream Pipeline Architecture]: 00dxyb6-query-stream-pipeline-architecture.md
[Structured Output Architecture]: 00dxyb7-structured-output-architecture.md
