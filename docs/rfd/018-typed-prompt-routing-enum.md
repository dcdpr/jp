# RFD 018: Typed Prompt Routing Enum

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01
- **Required by**: [RFD 049]
- **Summary**: Unifies permission prompts, result delivery prompts, and tool
  questions under a single `Prompt` enum for routing and configuration.

## Summary

This RFD introduces a `Prompt` enum that models permission prompts, result
delivery prompts, and tool questions as variants of a single type for unified
routing and configuration.

## Motivation

JP prompts the user at three points during tool execution:

1. **Permission prompts** ("Should I run this tool?"), constructed in
   `ToolPrompter::prompt_permission` using `PermissionInfo`.
2. **Result delivery prompts** ("Should I deliver this result?"), constructed in
   `ToolPrompter::prompt_result_confirmation` as an inline format string.
3. **Tool questions** ("Create backup?"), received as `Question` from
   `Outcome::NeedsInput`.

The coordinator handles each prompt kind on a separate code path, with no shared
type representing a prompt that needs an answer.
Scattering prompt definitions across the prompter, coordinator, and tool
executor makes it difficult to:

- Apply consistent routing logic (such as a detached fallback policy) across all
  prompt types.
- Configure behavior per prompt kind in a unified config schema.
- Discover every prompt type JP can emit in one place.
- Add new prompt types without duplicating routing logic.

## Design

### The `Prompt` Enum

All prompts are modeled as variants of a `Prompt` enum.
JP-level variants (`RunTool`, `DeliverToolResult`) are self-describing: their
answer type, exclusivity, and config key are derived from the variant.
`ToolQuestion` is the open-ended catch-all for tool-authored questions:

```rust
enum Prompt {
    /// "Should I execute this tool?" — before tool execution.
    /// Constructed by the coordinator. Tools cannot emit this.
    RunTool {
        tool_name: String,
        tool_source: ToolSource,
    },

    /// "Should I deliver this result to the LLM?" — after execution.
    /// Constructed by the coordinator. Tools cannot emit this.
    DeliverToolResult {
        tool_name: String,
    },

    /// Domain-specific question from a tool — during execution.
    /// Wraps the tool-authored Question from jp_tool::Outcome::NeedsInput.
    ToolQuestion {
        tool_name: String,
        question: Question,
    },
}
```

`Prompt` carries **data, not presentation**, leaving question text and ANSI
formatting to the rendering layer.

### Derived Properties

Methods on `Prompt` derive routing and configuration metadata from each variant:

```rust
impl Prompt {
    /// The expected answer type.
    fn answer_type(&self) -> AnswerType {
        match self {
            Self::RunTool { .. } | Self::DeliverToolResult { .. } => {
                AnswerType::Boolean
            }
            Self::ToolQuestion { question, .. } => {
                question.answer_type.clone()
            }
        }
    }

    /// Whether this prompt can only be answered by a human.
    ///
    /// RunTool and DeliverToolResult are always exclusive — the LLM
    /// cannot meaningfully answer "should I run the tool you just
    /// asked me to run?" Tool questions are non-exclusive by default,
    /// but can be overridden per-question in config.
    fn exclusive(&self) -> bool {
        match self {
            Self::RunTool { .. } | Self::DeliverToolResult { .. } => true,
            Self::ToolQuestion { .. } => false,
        }
    }

    /// Config key for policy lookup (used by RFD 049).
    fn config_key(&self) -> &str {
        match self {
            Self::RunTool { .. } => "run",
            Self::DeliverToolResult { .. } => "deliver",
            Self::ToolQuestion { .. } => "tool",
        }
    }

    /// The tool name associated with this prompt.
    fn tool_name(&self) -> &str {
        match self {
            Self::RunTool { tool_name, .. }
            | Self::DeliverToolResult { tool_name }
            | Self::ToolQuestion { tool_name, .. } => tool_name,
        }
    }
}
```

### Why Each Prompt Has Its Own Type

Defining JP-level prompts (`RunTool`, `DeliverToolResult`) at the type level
rather than at each call site provides:

1. **Discoverability**: Every prompt type JP can produce lives in a single enum
   definition.
2. **Uniformity**: Permission prompts (`build_permission_question`), result
   confirmations (`prompt_result_confirmation`), and tool questions (`Question`)
   use one representation instead of three separate patterns.
3. **Mechanical config mapping**: `config_key()` maps each variant directly to
   its key in the detached policy (see [RFD 049]).
4. **Type-level exclusivity**: `RunTool` and `DeliverToolResult` are always
   exclusive by type, rather than relying on callers to set a boolean flag.

Future JP-level prompts (such as `ConfirmEndConversation` or
`ApproveExpensiveModel`) add a variant with its own `config_key()`,
`exclusive()`, and `answer_type()` arms, extending the routing logic, config
cascade, and rendering mechanically.

### Tool Boundary

Tools can only produce `Outcome::NeedsInput { question }`, which the coordinator
wraps into `Prompt::ToolQuestion { tool_name, question }`.
A tool cannot construct `RunTool` or `DeliverToolResult` because its output type
(`Outcome`) does not include them and only the coordinator holds `ToolSource`.
This makes the boundary between tool-authored and coordinator-authored prompts
structural.

### Rendering

`Prompt` does not define question text.
Constructing a prompt's display text requires ANSI escapes
(`tool_name.yellow().bold()`), async MCP server resolution, and option lists
that depend on editor availability.
This presentation logic belongs in `ToolPrompter` (the CLI layer), which
pattern-matches on the variant to build styled output:

```rust
// In ToolPrompter — the rendering layer
fn render_prompt(&self, prompt: &Prompt, mcp_client: &Client) -> String {
    match prompt {
        Prompt::RunTool { tool_name, tool_source } => {
            // ANSI formatting, MCP resolution, source label
        }
        Prompt::DeliverToolResult { tool_name } => {
            format!("Deliver {} result to assistant?", tool_name.yellow().bold())
        }
        Prompt::ToolQuestion { question, .. } => {
            question.text.clone()
        }
    }
}
```

Different frontends (terminal, TUI, or JSON-over-IPC for `jp tasks attach`)
render the same `Prompt` differently without changing the enum.

### Prompt Routing

The coordinator constructs a `Prompt` and passes it to a central routing
function.
This RFD preserves existing routing behavior, where TTY detection drives the
decision:

```rust
fn route_prompt(prompt: &Prompt, has_client: bool) -> PromptAction {
    if has_client {
        return PromptAction::PromptClient;
    }

    // Current non-TTY behavior, now expressed through the enum.
    match prompt {
        Prompt::RunTool { .. } => PromptAction::AutoApprove,
        Prompt::DeliverToolResult { .. } => PromptAction::AutoDeliver,
        Prompt::ToolQuestion { .. } => PromptAction::LlmInquiry,
    }
}
```

[RFD 049] extends `route_prompt` with configurable detached policies.

## Drawbacks

- **Indirection.** Adding an enum layer between the coordinator and the prompter
  introduces abstraction over what currently works as direct function calls.
  The payoff comes from extensibility and config integration rather than
  immediate simplification.
- **Migration surface.** Refactoring `prompt_permission`,
  `prompt_result_confirmation`, and `prompt_question` to construct and route
  `Prompt` variants touches multiple files across the coordinator and prompter.

## Alternatives

### Keep prompt types implicit

Keeping the prompt type implicit in the active code path avoids introducing a
new type, but requires implementing every new routing policy in three separate
places.

### `text()` method on `Prompt`

Defining question text directly on `Prompt` would couple the data model to
terminal presentation, including ANSI escapes, async MCP server resolution, and
options that depend on editor availability.

### `exclusive` as a boolean field

Storing `exclusive` as a boolean field on each variant would allow constructing
invalid states such as `RunTool { exclusive: false }`.
Because `RunTool` and `DeliverToolResult` are inherently exclusive, a method
encodes the invariant in the type.

## Non-Goals

- **Detached prompt policy.** This RFD introduces the type system; configurable
  detached policies are proposed in [RFD 049].
- **Task model and prompt queuing.** Deferring a prompt rather than routing it
  immediately is a separate concern not yet covered by an RFD.
- **New prompt variants.** This RFD formalizes the three existing prompt types;
  additional variants are future work.

## Implementation Plan

### Phase 1: Add the `Prompt` enum

Define the `Prompt` enum and its methods in a dedicated module (such as
`jp_cli::cmd::query::tool::prompt`, or a shared crate if required by
configuration).
This phase introduces no behavioral changes and can merge independently.

### Phase 2: Refactor the coordinator

Depends on Phase 1.
Replace direct `PermissionInfo`, `ResultMode`, and `Question` handling in the
coordinator with `Prompt` construction and `route_prompt()` calls so the
prompter receives `Prompt` variants instead of raw data.
This refactoring is purely structural; TTY detection still drives routing.

### Phase 3: Refactor the prompter

Depends on Phase 2.
Update `ToolPrompter` to accept `Prompt` variants, consolidating
`prompt_permission`, `prompt_result_confirmation`, and `prompt_question` into a
pattern match on the enum while keeping rendering logic in the prompter.

## References

- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]: Extends
  `route_prompt()` with configurable detached policies.
- [RFD 028: Structured Inquiry System][RFD 028]: Defines the tool inquiry
  system, which is separate from prompt routing and handles `ToolQuestion`
  prompts routed to the LLM.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
