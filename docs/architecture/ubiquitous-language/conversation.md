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

> [!NOTE]
> Cluster status: only **Turn** is defined below.
> The remaining terms are placeholders and will land in subsequent passes.
> Until then, see the [legacy single-page glossary] for the older definitions of
> the unfilled terms.

## Terms

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
