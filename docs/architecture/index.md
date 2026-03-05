# Technical Architecture

This section describes the technical architecture of JP.

## Documents

- [JP Query Architecture](architecture.md) - Deep dive into the `jp query`
  command stack, including component hierarchy, data flow, state management,
  and refactoring opportunities for improved testability.

- [Query Stream Pipeline](query-stream-pipeline.md) - Target architecture for
  the query command's stream handling pipeline. Describes the Turn Coordinator
  state machine, Event Builder, Renderers, Tool Coordinator, and Interrupt
  Handler. This is the blueprint for refactoring the current implementation.

- [Structured Output](structured-output.md) - Architecture for unified
  structured output handling. Replaces the separate `structured_completion`
  code path with native provider APIs flowing through the standard streaming
  pipeline. The schema is an optional field on `ChatRequest`, and structured
  JSON data is a `Structured` variant on `ChatResponse`.

- [Knowledge Base](knowledge-base.md) - Architecture for the workspace
  knowledge base system. Describes topic/subject organization, system prompt
  injection, the `learn` tool, and CLI integration.

- [Wasm Tools](wasm-tools.md) - Architecture for executing tools as
  WebAssembly components. Covers the `wasmtime` runtime, WIT contract,
  builtin tools (embedded), local Wasm tools (disk-loaded), and the
  `jp_tool_learn` guest crate.
