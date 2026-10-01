# Let a plugin's query request ask for structured output

- **Status**: Done
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin

A plugin can run a turn through the host with `query`, but `QueryRequest`
carries only `conversation`, `content`, `new`, `title`, and `cfg`
(`crates/jp_plugin/src/message.rs:496-536`).
There is no way to ask for a response that matches a JSON Schema, or to run the
turn in a throwaway conversation that is not kept.

Input that hits it: RFD 116's `jp rfd track` reads an RFD's Implementation Plan
as structured output.
The recipe does it with `jp query --new --local --tmp=5m --format=json
--no-reasoning --no-tools --schema "$SCHEMA"` (`justfile:2703-2704`).
The plugin cannot ask its host for the same, so it shells out to `jp query` as a
subprocess.

The subprocess works and is cheap, so this is a cleanup, not a blocker.

## Scope

- `schema`: draft D18 (Plugin Event Subscriptions and Query Delegation) already
  specifies it on the `query` payload, as the same field `ChatRequest` carries.
  This ticket ships that field on its own, ahead of the rest of D18.
- A temporary conversation, the way `--local --tmp` gives one: not in D18.
  Without it, a plugin's one-off structured query leaves a conversation behind.

Reasoning and tools can likely be switched off through the existing `cfg` field;
check that before adding fields for them.

## Comments

-----

- **From**: jp
- **Date**: 2026-10-01T10:26:09Z

Implemented; protocol bumped to 11.

- `query.schema`: set on the turn's `ChatRequest`, as `jp query --schema` does.
  `query_complete` carries the parsed response in a new `data` field.
  A turn whose own response has no structured data is answered with an `error`;
  only the last turn is searched, so an earlier turn's data is never returned in
  its place.
- `query.expires_in`: a humantime duration (`5m`, `1h`, `0s`) stamped as
  `expires_at` on the conversation a `new` query creates.
  Refused without `new`, matching `--tmp` requiring `--new`.

The rest of the recipe's flags go through `cfg` on the same request (key paths
checked against the config tree, not exercised by a test):

- `--local`: `conversation.start_local=true`
- `--no-reasoning`: `assistant.model.parameters.reasoning=off`
- `--no-tools`: no exact equivalent.
  `assistant.tool_choice=none` forbids tool calls, but the tools are still sent
  with the request.
