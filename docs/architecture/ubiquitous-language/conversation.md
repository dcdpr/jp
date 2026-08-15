# Conversation

The Conversation cluster covers JP's central abstraction: the persistent record
of "talking to the assistant" and the event log that backs it.
Six terms work together here — a **Conversation** is the stored entity, a
**Turn** is one slice of it, an **Event** is the atomic unit inside a Turn,
**Tool Calls** and **Inquiries** are specific event kinds, and a **Thread** is
the projection of a Conversation that gets sent to an LLM provider.

These terms are tightly coupled — paraphrasing one usually breaks the model for
another.
Use them as written.

The cluster also covers a conversation's *state*: where it sits and how it is
annotated.
**Active**, **Live**, and **Archived** answer the first; **Label** answers the
second.

> [!NOTE]
> Cluster status: **Turn**, **Active Conversation**, **Live Conversation**,
> **Archived Conversation**, and **Label** are defined below.
> The remaining terms are placeholders and will land in subsequent passes.
> Until then, see the [legacy single-page glossary] for the older definitions of
> the unfilled terms.

## Terms

### Active Conversation

The one conversation a session is currently working on.
It is what `jp query` continues and what every conversation command targets when
given no explicit target.

Active is a property of the *session*, not of the conversation: two terminals
working in the same workspace each have their own active conversation, and a
conversation is not marked as active anywhere in its own metadata.

**Implementation.** The head of the session's activation history —
`SessionMapping::active_conversation_id`, reached through
`Workspace::session_active_conversation`.
The `.` / `active` target keyword resolves to it.

**Not the same as.** A **Live Conversation** (every conversation outside the
archive; the active one is a single member of that set).
Also not the session's *previous* conversation, which is the entry behind it in
the same history and answers to the `s` / `session` keyword.

### Live Conversation

A conversation in the default storage partition.
Live conversations are indexed when JP starts, so they are what every
whole-corpus operation sees without being asked: `jp c ls`, `jp c grep`, the
picker, and the `recent` / `newest` / `+pinned` target keywords.

**Implementation.** The `conversations/` partition of a storage root, scanned by
`LoadBackend::load_conversation_index` with a default `ConversationFilter`.
`Workspace::conversations()` iterates the resulting index.

**In context.** Live and **Archived** are the two values of a conversation's
*location* — mutually exclusive, and a conversation is always in exactly one.
This is a separate axis from flags such as **Pinned**, which apply to a
conversation in either location.

**Not the same as.** The **Active Conversation** (the single conversation the
current session is working on, which is one particular live conversation).

**Avoid.** *Non-archived*, *unarchived*, *indexed*.
The first two define the term by negation, which stops working as more locations
appear; the third names a storage strategy rather than a state.

### Archived Conversation

A conversation moved out of the default partition into the archive, to keep it
out of listings and whole-corpus operations without deleting it.

Archived conversations are deliberately **not** indexed at startup.
That is what keeps `jp c ls`, sanitization, and the picker fast in a workspace
where most conversations have been archived, and it is why reaching one costs an
explicit target (`a`, `?a`, `+a`) or `--archived`.

**Implementation.** The `conversations/.archive/` partition, reached via
`ConversationFilter { archived: true }`.
`Conversation::archived_at` records when it happened, but the directory location
is the source of truth.

**Not the same as.** A removed conversation (archiving is reversible with `jp c
unarchive`; removal is not).

### Label

A `key=value` annotation on a conversation, used to find it later by the context
it was created in: the VCS branch, the team, a review stage.
A label with an empty value is a *bare* label, and filters treat it as "key
present, any value".

Labels live in two places with distinct roles.
The **rules** that produce them are configuration, declared under
`conversation.labels.<key>` and layered like any other config; a rule's value is
a literal string, or a command whose trimmed stdout becomes the value.
The **resolved set** is the labels themselves, written at creation, on fork, and
by the CLI.
A rule is not a label until it has been resolved.

**Implementation.** `Conversation::labels` in `jp_conversation`, a
`BTreeMap<String, String>` persisted in `metadata.json`.
Rules are `LabelConfig` in `jp_config::conversation::label`, turned into the map
by the resolver in `jp_cli::cmd::label::resolve`.
Keys match `[A-Za-z][A-Za-z0-9_-]*`; every excluded character is significant
somewhere else — `.` separates dotted config paths, `=` splits a key from its
value, `:` marks a rule reference, and a leading `-` would read as a flag where
keys are written as bare command arguments.
Values carry no such restriction.

**In context.** A key holds exactly one value, so setting the same key twice
replaces rather than accumulates, and removal names a key rather than a pair.
Labels are a fact recorded *about* a conversation: they are not sent to the LLM
and not exposed to tools.

**Not the same as.** A conversation's **title**, which is free-form, singular,
and serves as the conversation's name rather than a fact about it.
Also not an **Attachment**, which is content added *to* a conversation.

**Avoid.** *Tag*, *annotation*, *marker*.
When you mean a `key=value` pair stored on a conversation, the word is
**Label**.

### Turn

A contiguous group of conversation events bracketed by a `TurnStart`: one user
chat request through the assistant's final response for that request, including
any intermediate tool calls and inquiries.

**Implementation.** `Turn<'a>` in `jp_conversation::stream::turn_iter`.
Constructed by iterating a `ConversationStream` and splitting on `TurnStart`
events.

**In context.** A **Conversation** is an ordered sequence of Turns.
Each Turn contains a **ChatRequest** and the **Events** that flow from it —
typically one or more **ChatResponses** from the assistant, optionally
interleaved with **Tool Calls** and **Inquiries**.
A **Thread** is assembled across many Turns at query time; a Thread is *not* a
Turn.

**Not the same as.** A Conversation (a Conversation *contains* Turns), a Thread
(a provider-facing projection assembled across many Turns), a Tool Call (a Tool
Call is one Event within a Turn).

**Avoid.** *Round*, *exchange*, *message*, *interaction*.
None of these are project terms.
When you mean a single user-prompt-to-final-response cycle with the assistant,
the word is **Turn**.

[legacy single-page glossary]: ../ubiquitous-language.md
