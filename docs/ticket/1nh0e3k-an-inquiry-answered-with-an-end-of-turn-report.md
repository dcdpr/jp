# An inquiry answered with an end-of-turn report

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: relevant-rfd=034
- **Label**: type=bug

When `ticket_create` prompted for a shorter title ("Shorter title, at most 60
characters?"), the inquiry model returned a 2,235-character plain-text report of
the turn's work inside the structured `answer` field, ending with "A
44-character title would work: …".
`ticket_create` rejected the oversized answer, and the primary turn model then
retried `ticket_create` with a valid title.

The conversation was running the `rfd-implementor` persona, whose instructions
require ending every turn with a summary report (the user had requested plain
text instead of the default Markdown sections).

## Why the persona instruction reaches the inquiry

- `build_inquiry_backend` (`crates/jp_cli/src/cmd/query/turn_loop.rs`) builds
  the inquiry system prompt from `conversation.inquiry.assistant`, which
  inherits unset fields from `assistant` ([RFD 034]), including the active
  persona's `instructions`.
  This workspace runs inquiries on `haiku`.
- `LlmInquiryBackend::inquire` (`crates/jp_cli/src/cmd/query/tool/inquiry.rs`)
  appends the inquiry question as a new turn (`events.start_turn`), matching the
  persona instruction's trigger ("End every turn with...").
- `inquiry_prompt` asks for "only the answer to this question", competing
  directly with the system prompt instruction.
- `Question::text` cannot express a maximum length in JSON Schema, so the schema
  is `{"answer": {"type": "string"}}` and the 60-character limit exists only in
  the question text.

## Mitigations in this workspace

- `.jp/config.toml` defines `conversation.inquiry.assistant.instructions`,
  overriding the persona's `instructions` during inquiries instead of appending
  to them.
- `ticket_create` truncates rejected title answers in its error message to the
  first line (up to twice the character limit).

## Host options

1. Clarify in `inquiry_prompt` that answering a tool inquiry is not the end of a
   conversation turn, so end-of-turn reporting instructions do not apply.
2. Add an optional length bound to `jp_tool::AnswerType::Text` so the host can
   include `maxLength` in the inquiry JSON Schema and validate the response
   before re-running the tool.
   This requires a plugin protocol update; `ticket_create` would set the limit.
3. Stop inheriting `assistant.instructions` into
   `conversation.inquiry.assistant` by default while still inheriting
   `system_prompt_sections` (partially reversing [RFD 034]).

Option 2 enforces length constraints at the schema and host level; option 1 is a
low-cost prompt guard for unconstrained text questions.
