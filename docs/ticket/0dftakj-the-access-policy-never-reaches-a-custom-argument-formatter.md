# The access policy never reaches a custom argument formatter

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-04
- **Implements**: 076

A local tool with `style.parameters = "<command>"` runs that command with a
`Context` whose `access` field is absent, so it deserializes as `None` —
indistinguishable from "this tool has no access grants".

`format_args_custom` builds the context JSON by hand
(`crates/jp_cli/src/render/tool.rs:812-823`) with `action`, `root`,
`workspace_id`, and `conversation_id`, and nothing else.
The `Run` path does plumb the compiled policy (`execute_local` takes `access:
Option<&jp_tool::AccessPolicy>`, `crates/jp_llm/src/tool.rs:832`), so the same
tool binary sees its grants for one action and not the other.

RFD 076 Phase 3 covers both halves and only one shipped:

> Include access policy in the JSON context passed to tool commands in
> `execute_local()` and the `FormatArguments` path.

## What this is not

Enforcement is cooperative.
`run_tool_command` never clears the environment
(`crates/jp_llm/src/tool.rs:461`), so a local tool binary can read any variable
with `std::env::var` on either path, policy or no policy.
Passing the policy to the formatter does not create a boundary that isn't there
on the `Run` path.

The defect is narrower and worth fixing on its own terms: a tool author who
*wants* their formatter to honour the grants it was given has no way to read
them.
A formatter that renders a preview of what a call will touch cannot say "this
variable is denied" without them.

## Scope

- Compile the policy once and pass it into both context constructions, rather
  than compiling it twice with two chances to diverge.
- A test covering both `Action::Run` and `Action::FormatArguments` on one tool
  with one policy, asserting the `access` field is present and equal in both.

Predates `access.env`: `access.fs` has had the same gap since `ab94ec0a`
(\#727).
