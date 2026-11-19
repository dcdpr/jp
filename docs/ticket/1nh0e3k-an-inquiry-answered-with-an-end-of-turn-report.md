# An inquiry answered with an end-of-turn report

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: relevant-rfd=034
- **Label**: type=bug

Asked by `ticket_create` for a shorter title ("Shorter title, at most 60
characters?"), the inquiry model answered with 2235 characters: a plain-text
report of the turn's work, ending with "A 44-character title would work: …",
all inside the structured `answer` field.
The ticket tool refused it, and the turn's own model filed the ticket again with
a shorter title.

The conversation ran the `rfd-implementor` persona, whose instructions say "End
every turn with a markdown report"; the user had asked for plain text instead of
the formal sections.
The inquiry's request couldn't be inspected, so the report's shape is the
evidence that the model followed that instruction.

## Why the instruction reaches the inquiry

- `build_inquiry_backend` (`crates/jp_cli/src/cmd/query/turn_loop.rs`) builds
  the inquiry's sections from `conversation.inquiry.assistant`, which takes
  undeclared keys from `assistant` ([RFD 034]), so a persona's instructions
  reach every inquiry.
  This workspace answers inquiries with `haiku`.
- `LlmInquiryBackend::inquire` (`crates/jp_cli/src/cmd/query/tool/inquiry.rs`)
  puts the question as a new turn (`events.start_turn`), which is what the
  persona's instruction is about.
- `inquiry_prompt` asks for "only the answer to this question", against a system
  prompt that says how every turn ends.
- A text question can't state a length limit, so the answer schema is
  `{"answer": {"type": "string"}}`, and only the question's prose holds the
  answer to 60 characters.

## Mitigated in this workspace

- `.jp/config.toml` declares `conversation.inquiry.assistant.instructions`,
  which replaces the persona's instructions for inquiries instead of adding to
  them.
- `ticket_create` quotes an unusable answer only as far as its first line, cut
  at twice the limit.

## Options for the host

1. Say in `inquiry_prompt` that the question doesn't end a turn, so instructions
   about how a turn ends don't apply.
   Cheap, and unproven against a model.
2. Let a text question carry a length limit in `jp_tool`'s `AnswerType::Text`.
   The host puts it in the schema (`maxLength`) and checks the answer before
   re-running the tool, asking once more with the reason when it doesn't fit.
   A protocol change for plugins; `ticket_create` would set the limit.
3. Stop inheriting `instructions` into inquiries by default, keeping
   `system_prompt_sections`.
   That reverses part of [RFD 034], which carries the whole assistant through on
   purpose.

Option 2 fixes the failure for any question with a checkable constraint; option
1 is the cheap guard for the rest.
