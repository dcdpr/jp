# Ubiquitous Language

This is JP's domain vocabulary: the shared, rigorous terms used across code,
documentation, commits, RFDs, CLI help, and error messages. Every contributor
(human or AI) should use these terms *as written* — don't paraphrase or
substitute near-synonyms.

When you encounter a new concept that doesn't fit existing terms, add it here.
When an existing term is contradicted by usage or misleading, update the
definition — don't paper over the drift with aliases or inline comments
explaining the mismatch.

In disagreements between code and docs, the code is authoritative.

## Table of Contents

<!--toc:start-->
- [Ubiquitous Language](#ubiquitous-language)
  - [Table of Contents](#table-of-contents)
  - [Terms](#terms)
    - [Attachment](#attachment)
    - [Conversation](#conversation)
    - [Conversation Event](#conversation-event)
    - [Inquiry](#inquiry)
    - [Persona](#persona)
    - [Provider](#provider)
    - [RFD](#rfd)
    - [Thread](#thread)
    - [Tool Call](#tool-call)
    - [Turn](#turn)
    - [Workspace](#workspace)
<!--toc:end-->

## Terms

### Attachment

External content attached to a conversation to provide context: a file, URL
contents, command output, Bear note, MCP resource, etc. Implemented as
`Attachment` in `jp_attachment`. Each attachment kind is a separate crate
(`jp_attachment_file_content`, `jp_attachment_cmd_output`, and so on).

### Conversation

A persistent sequence of events identified by a `ConversationId`, living within
a Workspace. Implemented as `ConversationStream` in `jp_conversation`. The
user-facing notion of "a chat history with the assistant."

**Not to be confused with Thread.** A Conversation is the stored entity; a
Thread is what we build from it to send to an LLM.

### Conversation Event

The atomic unit of a conversation. Implemented as `ConversationEvent` (with
`EventKind`) in `jp_conversation`. The variants are `TurnStart`, `ChatRequest`,
`ChatResponse`, `ToolCallRequest`, `ToolCallResponse`, `InquiryRequest`,
`InquiryResponse`.

Not every event is sent to LLM providers. `EventKind::is_provider_visible()`
filters the stream down to the chat and tool-call events; turn markers and
inquiries are internal.

### Inquiry

A structured question-and-answer pair between the assistant, a tool, and/or the
user — distinct from a regular chat message. Carried as `InquiryRequest` and
`InquiryResponse` events within a conversation. Used for mid-turn clarification
that should not appear in the main chat stream or be sent to the LLM provider as
context.

### Provider

An LLM vendor integration — one of `anthropic`, `google`, `openai`,
`openrouter`, `llamacpp`, `ollama`, `cerebras`, `deepseek`. Each implements the
`Provider` trait in `jp_llm`.

### RFD

"Request for Discussion" — JP's design document format, stored in `docs/rfd/`.
Each RFD captures design rationale for a significant change. Numeric-prefixed
RFDs (`001-`, `002-`, …) are the accepted series; `D`-prefixed RFDs (`D01-`,
`D02-`, …) are drafts or abandoned proposals. The process itself is defined in
[RFD-001](../rfd/001-jp-rfd-process.md).

### Thread

The decomposed, provider-facing projection of a Conversation: a rendered system
prompt, rendered instruction sections, raw attachments, and a filtered event
stream, ready to be sent to an LLM provider. Implemented as `Thread` in
`jp_conversation::thread`.

A Conversation becomes a Thread at query time, via the config and conversation
pipeline. A Thread is transient; a Conversation is persisted.

### Tool Call

An LLM-requested function invocation (`ToolCallRequest`) and its eventual
response (`ToolCallResponse`). Tool calls are events within a Turn. The tool
itself can be a built-in, a local command, an MCP-provided tool, or a plugin.

### Turn

A group of conversation events delimited by a `TurnStart` marker: one user chat
request through the assistant's final response for that request, including any
intermediate tool calls and inquiries. Implemented as `Turn<'a>` in
`jp_conversation::stream::turn_iter`.

A single Conversation contains many Turns, separated by `TurnStart` events.

### Workspace

The top-level project unit, housing conversations, configuration, plugins, and
state for JP. Identified by a `.jp/` directory at the project root. Implemented
as `Workspace` in `jp_workspace`.
