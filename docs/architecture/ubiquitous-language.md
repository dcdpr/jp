# Ubiquitous Language

This is JP's domain vocabulary: the shared, rigorous terms used across code,
documentation, commits, RFDs, CLI help, and error messages.
Every contributor (human or AI) should use these terms *as written* — don't
paraphrase or substitute near-synonyms.

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
    - [Background Task](#background-task)
    - [CommandConfig](#commandconfig)
    - [Compacted View](#compacted-view)
    - [Compaction](#compaction)
    - [Compaction Rule](#compaction-rule)
    - [Conversation](#conversation)
    - [Conversation Event](#conversation-event)
    - [EditorBackend](#editorbackend)
    - [InlineReply](#inlinereply)
    - [Inquiry](#inquiry)
    - [Match](#match)
    - [Persona](#persona)
    - [Pinned Conversation](#pinned-conversation)
    - [Provider](#provider)
    - [RFD](#rfd)
    - [Search Hit](#search-hit)
    - [Signal Router](#signal-router)
    - [Summary](#summary)
    - [Thread](#thread)
    - [Tool Call](#tool-call)
    - [Turn](#turn)
    - [Workspace](#workspace)
    - [Workspace Projection](#workspace-projection)

<!--toc:end-->

## Terms

### Attachment

External content attached to a conversation to provide context: a file, URL
contents, command output, Bear note, MCP resource, etc. Implemented as
`Attachment` in `jp_attachment`.
Each attachment kind is a separate crate (`jp_attachment_file_content`,
`jp_attachment_cmd_output`, and so on).

### Background Task

Async work scheduled to run alongside a command and committed to the workspace
before JP exits.
Implemented by the `Task` trait and `TaskHandler` in `jp_task`.
A task is two-phase: `run` (off the main critical path, cancellable, accumulates
state) and `sync` (with `&mut Workspace`, commits the accumulated state).
The handler lives at `Ctx::task_handler` and is drained exactly once per `jp`
invocation, at the end of `jp_cli::lib::run`.

### CommandConfig

The shape of an external command JP runs on behalf of the user: a program,
arguments, and an optional `shell` flag.
Implemented as `CommandConfig` in `jp_config::types::command`, with
`CommandConfigOrString` providing a string-shorthand variant for TOML/JSON
config.

String-shorthand values (`command = "git log --oneline"`) are parsed with
shell-word semantics via [`shlex::split`], so quoting works (`"echo 'hello
world'"` is one `hello world` arg).
Malformed shell quoting is rejected at config-parse time.

Consumers (tools, conversation labels, the editor env-var fallback, the `cmd:`
attachment URL) share the same shape and parser.
The policy around *when* JP is allowed to run a `CommandConfig` (prompt or not,
confirm `shell = true` invocations) lives on each consumer, not on the shape
itself.

### Compacted View

What the LLM actually receives for a conversation: the raw event stream with
every [Compaction](#compaction) overlay applied.
Produced by `ConversationStream::apply_projection` in `jp_conversation`, which
also returns a `TurnOrigin` per resulting turn mapping it back to the raw turn
number(s) it stands for.
`jp conversation print --compacted` renders it.

Turn numbering differs between the two: a [Summary](#summary) collapses its
range into a single turn, so the compacted view can be shorter than the
conversation it came from.

**Not the same as** a [Workspace Projection](#workspace-projection).
The word "projection" carries two unrelated meanings in the codebase: applying
compaction overlays (`jp_conversation::stream::projection`) and writing a
conversation into the workspace directory (`Projection` in `jp_storage`).

### Compaction

A non-destructive overlay that reduces what the provider sees for an inclusive
range of turns.
The original events are never removed: the overlay is appended to the
conversation stream and applied when the [Compacted View](#compacted-view) is
built.
Implemented as `Compaction` in `jp_conversation::compaction`, carrying up to
three independent policies over its range — a [Summary](#summary), a reasoning
policy, and a tool-call policy.
See [RFD-064].

A Summary supersedes the other two for the turns it covers.

### Compaction Rule

The configuration that produces a [Compaction](#compaction): how many turns to
preserve at each end, and which policies to apply to the rest.
Implemented as `CompactionRuleConfig` in `jp_config::conversation::compaction`;
each rule yields exactly one Compaction when applied.

**Not the same as** a Compaction.
A rule is durable configuration in relative terms ("keep the last turn"); a
Compaction is the event it produced, pinned to absolute turn indices and stored
in the conversation.

### Conversation

A persistent sequence of events identified by a `ConversationId`, living within
a Workspace.
Implemented as `ConversationStream` in `jp_conversation`.
The user-facing notion of "a chat history with the assistant."

**Not to be confused with Thread.** A Conversation is the stored entity; a
Thread is what we build from it to send to an LLM.

### Conversation Event

The atomic unit of a conversation.
Implemented as `ConversationEvent` (with `EventKind`) in `jp_conversation`.
The variants are `TurnStart`, `ChatRequest`, `ChatResponse`, `ToolCallRequest`,
`ToolCallResponse`, `InquiryRequest`, `InquiryResponse`.

Not every event is sent to LLM providers.
`EventKind::is_provider_visible()` filters the stream down to the chat and
tool-call events; turn markers and inquiries are internal.

### EditorBackend

The frontend seam for invoking the user's configured editor.
Exposes `edit_text` (string in, edited string out) and `edit_file` (open the
editor on caller-owned paths), covering both editing shapes a frontend offers.
Each frontend is one implementation: `TerminalEditorBackend` spawns the editor
as a local process via `EditorConfig::command()`, and `MockEditorBackend`
scripts outcomes for tests.
Defined as the `EditorBackend` trait in `jp_editor`; call sites obtain one
through `build_editor_backend` in `jp_cli`.

### InlineReply

The `jp_inquire` widget for short replies: the interrupt-menu reply (`r` in both
the streaming and tool menus) and the tool argument / result / skip-reason
edits.
It renders on the `/dev/tty` prompt writer and accepts inline typing, with a
`Ctrl+X` escape to the configured editor (the `EditorBackend`) for longer edits.
Submitting produces a `ReplyOutcome`; the call site decides what an empty
submission or a `Ctrl+C` cancel means.
Built on the vendored reedline at `crates/contrib/reedline`; the inline buffer's
editing style is set by `editor.inline.edit_mode`.

### Inquiry

A structured question-and-answer pair between the assistant, a tool, and/or the
user — distinct from a regular chat message.
Carried as `InquiryRequest` and `InquiryResponse` events within a conversation.
Used for mid-turn clarification that should not appear in the main chat stream
or be sent to the LLM provider as context.

### Match

A [Search Hit](#search-hit) whose line actually contains the pattern, as opposed
to one included only for surrounding context.
Recorded as `is_match` on the hit, along with the byte ranges of the matched
substrings.

Matches are the unit every count in `jp conversation grep` uses: the figure in a
heading, the `--output count` value, and the `--max-matches` cap.

**Not the same as.** A Search Hit, which also covers context lines.

### Pinned Conversation

A conversation the user has marked as important, so it stays prominent and is
protected from casual removal.
Pinning is a property of the conversation itself, not of any session or view: it
persists with the conversation and means the same thing everywhere the
conversation appears.
Persisted as a `pinned_at` timestamp on the conversation metadata;
`Conversation::is_pinned()` in `jp_conversation` is the predicate.

**Not the same as** binding a session to a particular target.
Pinning marks one conversation as important; it does not make a session "keep
using" that conversation.
Keeping a session attached to a chosen target is a separate, per-session
concept, not conversation pinning.

### Provider

An LLM vendor integration — one of `anthropic`, `google`, `openai`,
`openrouter`, `llamacpp`, `ollama`, `cerebras`, `deepseek`.
Each implements the `Provider` trait in `jp_llm`.

### RFD

"Request for Discussion" — JP's design document format, stored in `docs/rfd/`.
Each RFD captures design rationale for a significant change.
Numeric-prefixed RFDs (`001-`, `002-`, …) are the accepted series; `D`-prefixed
RFDs (`D01-`, `D02-`, …) are drafts or abandoned proposals.
The process itself is defined in [RFD-001].

### Search Hit

One line of conversation content emitted by a search, together with the
coordinate that locates it: the conversation, the turn it came from, and which
part of the conversation it was found in (its scope — title, user, assistant,
reasoning, tool call, and so on).
Implemented as `Hit` in `jp_cli::cmd::conversation::grep`.

A hit is either a [Match](#match) or a context line pulled in by `--context`;
both are hits.

**Not the same as.** A Match (a hit whose line contains the pattern), a
Conversation Event (one hit is a single line from within an event, and one event
can yield many hits).

### Signal Router

The process-wide owner of OS signal handling: `SignalRouter` in
`jp_cli::signals` (RFD 045).
It consumes SIGINT/SIGTERM/SIGQUIT once, tracks Ctrl+C **escalation** (first
press → topmost interrupt handler, second press within the cooldown → graceful
shutdown, any press after shutdown began → immediate exit), and owns the root
**shutdown token** — a `CancellationToken` cancelled when a graceful shutdown
is requested, observed cooperatively by teardown and long-running work.

Scopes that can act on a Ctrl+C register an **interrupt handler** on the
router's LIFO stack via `push_handler`, receiving an RAII guard and a
notification channel polled from their own event loop.
Only the topmost handler is notified; a handler may `decline` to pass the
interrupt down the stack.
The registered scopes are the streaming loop, the tool execution loop, and the
turn-level handler covering gaps between turn phases.

### Summary

Text that stands in for a range of turns in the [Compacted
View](#compacted-view): the turns it covers collapse into a single synthetic
request/response pair carrying the text.
Attached to a [Compaction](#compaction) as `SummaryPolicy` in
`jp_conversation::compaction`, whose `SummarySource` records whether the text
was *generated* (produced by a model reading the raw events in the range) or
*authored* (supplied verbatim by the user).

The distinction is operational: a generated summary can be re-derived for a
wider range, an authored one cannot.

**Not the same as** the mechanical compaction policies (reasoning stripping,
tool-call stripping), which filter events within their range rather than
replacing the range.

### Thread

The decomposed, provider-facing projection of a Conversation: a rendered system
prompt, rendered instruction sections, raw attachments, and a filtered event
stream, ready to be sent to an LLM provider.
Implemented as `Thread` in `jp_conversation::thread`.

A Conversation becomes a Thread at query time, via the config and conversation
pipeline.
A Thread is transient; a Conversation is persisted.

### Tool Call

An LLM-requested function invocation (`ToolCallRequest`) and its eventual
response (`ToolCallResponse`).
Tool calls are events within a Turn.
The tool itself can be a built-in, a local command, an MCP-provided tool, or a
plugin.

### Turn

A group of conversation events delimited by a `TurnStart` marker: one user chat
request through the assistant's final response for that request, including any
intermediate tool calls and inquiries.
Implemented as `Turn<'a>` in `jp_conversation::stream::turn_iter`.

A single Conversation contains many Turns, separated by `TurnStart` events.

### Workspace

The top-level project unit, housing conversations, configuration, plugins, and
state for JP.
Identified by a `.jp/` directory at the project root.
Implemented as `Workspace` in `jp_workspace`.

### Workspace Projection

The copy of a conversation written into the workspace's `.jp/conversations/`
directory so it can be committed to version control alongside the project.
The durable source of truth is the user-local copy; the workspace copy is a
*projection* of it.
Non-local conversations are projected (written to both roots); `--local`
conversations live only in user-local storage and have no projection.
The write intent is carried by a conversation's lock (`Projection` in
`jp_storage`) and derived at load time from where the data lives
(`StoragePresence`).
See [RFD-031].

[RFD-001]: ../rfd/001-jp-rfd-process.md
[RFD-031]: ../rfd/031-durable-conversation-storage-with-workspace-projection.md
[RFD-064]: ../rfd/064-non-destructive-conversation-compaction.md
[`shlex::split`]: https://docs.rs/shlex
