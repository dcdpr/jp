# RFD 018: Typed Prompt Routing Enum

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01

## Summary

This RFD introduces a `Prompt` enum that codifies all prompt types in JP as
typed variants, replacing the current implicit code-path-based distinction
between permission prompts, result delivery prompts, and tool questions.

## Motivation

JP has three kinds of prompts during tool execution:

1. **Permission prompts** — "Should I run this tool?" Constructed in
   `ToolPrompter::prompt_permission` using `PermissionInfo`.
2. **Result delivery prompts** — "Should I deliver this result?" Constructed in
   `ToolPrompter::prompt_result_confirmation` as an inline format string.
3. **Tool questions** — "Create backup?" Received as `Question` from
   `Outcome::NeedsInput`.

These are handled by three separate code paths in the coordinator, with no
shared type representing "a prompt that needs an answer." This makes it
difficult to:

- Apply consistent routing logic (e.g., a detached fallback policy) across all
  prompt types.
- Configure behavior per prompt kind in a unified config schema.
- Discover which prompt types exist — they are scattered across the prompter,
  coordinator, and tool executor.
- Extend the system with new prompt types without duplicating routing logic.

## Design

### The `Prompt` Enum

All prompts are modeled as variants of a `Prompt` enum. JP-level variants are
fully self-describing (their answer type, exclusivity, and config key are
derived from the variant). Tool questions are the open-ended catch-all.

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

The enum carries **data, not presentation**. It does not define question text or
ANSI formatting. The prompter (CLI layer) pattern-matches on the variant and
renders the appropriate styled output. This keeps the enum usable by different
frontends (terminal, JSON-over-IPC, etc.).

### Derived Properties

Methods on the enum derive properties from the variant:

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

JP-level prompts (`RunTool`, `DeliverToolResult`) are defined at the type
level, not at the call site. This provides:

1. **Discoverability** — one enum, one file. Every prompt type JP can produce is
   visible in one place.
2. **No duplicates** — today the permission question text is constructed in
   `build_permission_question`, the result text in `prompt_result_confirmation`,
   and tool questions come from `Question`. Three patterns for the same concept.
3. **Mechanical config mapping** — `config_key()` maps directly to config keys
   in the detached policy (see [RFD 049]).
4. **Type-level exclusivity** — `RunTool` and `DeliverToolResult` are always
   exclusive because the type says so, not because someone remembered to set a
   boolean.

Future JP-level prompts (e.g., `ConfirmEndConversation`,
`ApproveExpensiveModel`) add a variant with its own `config_key()`,
`exclusive()`, and `answer_type()`. The routing logic, config cascade, and
rendering extend mechanically.

### Tool Boundary

Tools can only produce `Outcome::NeedsInput { question }`. The coordinator wraps
that into `Prompt::ToolQuestion { tool_name, question }`. A tool cannot
construct `RunTool` or `DeliverToolResult` because those variants require data
(`ToolSource`) that only the coordinator has, and the tool's output type
(`Outcome`) does not include them. The type boundary is structural.

### Rendering

The enum does not define question text. Question text today includes ANSI
escapes (`tool_name.yellow().bold()`), async MCP server resolution, and
editor-availability-dependent option lists. This is rendering logic that belongs
in the prompter (CLI layer).

The prompter pattern-matches on the variant and builds styled text:

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

Different frontends (terminal, TUI, JSON-over-IPC for `jp tasks attach`) render
the same `Prompt` differently without touching the enum.

### Prompt Routing

The coordinator constructs a `Prompt` and passes it to a central routing
function. In this RFD, routing preserves existing behavior — TTY detection still
drives the decision:

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

[RFD 049] extends this function with configurable detached policies.

## Drawbacks

**Indirection.** Adding an enum layer between the coordinator and the prompter
is more abstraction for what currently works as direct function calls. The
payoff is in extensibility and config integration, not in immediate
simplification.

**Migration surface.** Refactoring three code paths (`prompt_permission`,
`prompt_result_confirmation`, `prompt_question`) to construct and route `Prompt`
variants touches multiple files in the coordinator and prompter.

## Alternatives

### Keep prompt types implicit

Continue with the current approach where prompt type is determined by which code
path you're in. Rejected because it makes future improvements significantly
harder — every new routing policy would need to be implemented in three separate
places.

### `text()` method on the Prompt enum

Have the enum define the question text directly. Rejected because question text
includes ANSI escapes, async MCP server resolution, and
editor-availability-dependent options. This is presentation, not data.

### `exclusive` as a boolean field instead of a method

Store `exclusive` as a field on every variant. Rejected because `RunTool` and
`DeliverToolResult` are inherently exclusive — making it a field would allow
constructing an impossible state (`RunTool { exclusive: false }`). The method
encodes the invariant in the type.

## Non-Goals

- **Detached prompt policy.** This RFD introduces the type system; configurable
  detached policies are proposed in [RFD 049].
- **Task model and prompt queuing.** See [RFD 020].
- **New prompt variants.** This RFD formalizes the existing three prompt types.
  New variants are future work.

## Implementation Plan

### Phase 1: Add the `Prompt` enum

Define the `Prompt` enum and its methods. Likely in a new module (e.g.,
`jp_cli::cmd::query::tool::prompt` or a shared crate if needed by config).

Can be merged independently. No behavioral changes.

### Phase 2: Refactor the coordinator

Replace direct `PermissionInfo` / `ResultMode` / `Question` handling in the
coordinator with `Prompt` construction and `route_prompt()` calls. The prompter
receives `Prompt` variants instead of raw data.

Behavior is unchanged — TTY detection still drives routing. The refactor is
purely structural.

Depends on Phase 1.

### Phase 3: Refactor the prompter

Update `ToolPrompter` to accept `Prompt` variants. Consolidate
`prompt_permission`, `prompt_result_confirmation`, and `prompt_question` into a
pattern-match on the enum. The rendering logic stays in the prompter.

Depends on Phase 2.

## References

- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049] — extends
  `route_prompt()` with configurable detached policies.
- [RFD 028: Structured Inquiry System][RFD 028] — the tool inquiry system (a
  separate concept from prompt routing; handles `ToolQuestion` prompts routed to
  the LLM).

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
