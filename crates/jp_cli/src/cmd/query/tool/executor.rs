//! The seam a turn loop runs one tool call through.
//!
//! [`Executor`] is the MCP Host's view of one logical tool call: preparation
//! and approval precede execution release, and an input request returns control
//! to the Host so it can route the inquiry, review the result, and record both.
//! [`ExecutorSource`] builds one per tool call, so a test can supply a scripted
//! executor where production supplies [`super::mcp_executor`].
//!
//! Execution itself lives in `jp_mcp::server`; nothing here runs a tool.

use async_trait::async_trait;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use jp_config::conversation::tool::{RunMode, ToolConfigWithDefaults, ToolSource};
use jp_conversation::event::{InquirySource, ToolCallRequest, ToolCallResponse};
use jp_llm::query::ToolExecution;
use jp_mcp::server::StderrSink;
use jp_tool::{Question, ToolResult};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

#[path = "executor_error.rs"]
mod error;
pub(crate) use error::ExecutorError;

/// The MCP Host's view of a logical tool call.
///
/// Each step replies to whatever the call is parked on and returns the next
/// thing it needs from the Host: a question answered, an admission decision, a
/// release, or a result reviewed and recorded.
/// A question is answered the same way whichever step raised it, the tool's or
/// its argument formatter's: with [`execute`] and the answers so far.
///
/// [`execute`]: Self::execute
#[async_trait]
pub(crate) trait Executor: Send + Sync {
    /// Submit the call, and return the first thing it needs from the Host.
    ///
    /// `render_arguments` is whether the Host wants the argument formatter's
    /// description of the call.
    async fn prepare(
        &self,
        _render_arguments: bool,
        _cancellation: CancellationToken,
    ) -> ExecutorResult {
        ExecutorResult::AwaitingAdmission
    }

    /// Admit the call with its current arguments, and return the next thing it
    /// needs from the Host.
    async fn approve(&self, _cancellation: CancellationToken) -> ExecutorResult {
        ExecutorResult::AwaitingRelease
    }

    /// Custom argument rendering provided by the execution service.
    ///
    /// `None` when the execution service has not formatted this call's
    /// arguments, either because nothing asked it to or because its formatter
    /// waits for admission.
    fn formatted_arguments(&self) -> Option<String> {
        None
    }

    /// Returns the tool call ID.
    fn tool_id(&self) -> &str;

    /// Returns the tool name.
    fn tool_name(&self) -> &str;

    /// Returns the tool call arguments.
    ///
    /// This is separate from [`permission_info()`] because arguments are always
    /// available, while permission info is only present for tools that require
    /// a permission prompt.
    ///
    /// [`permission_info()`]: Self::permission_info
    fn arguments(&self) -> Map<String, Value>;

    /// Returns information needed for permission prompting.
    ///
    /// Returns `None` if the tool doesn't need a permission prompt (e.g.,
    /// `RunMode::Unattended` or `RunMode::Skip`).
    fn permission_info(&self) -> Option<PermissionInfo>;

    /// Whether this call needs a permission prompt before it runs.
    ///
    /// Agrees with [`permission_info()`] being `Some`, without copying the
    /// arguments to find out.
    ///
    /// [`permission_info()`]: Self::permission_info
    fn needs_permission(&self) -> bool {
        self.permission_info().is_some()
    }

    /// Updates the arguments to use for execution.
    ///
    /// This is called after permission prompting if the user edited the
    /// arguments (via `RunMode::Edit`).
    /// The new arguments replace the original arguments from the tool call
    /// request.
    fn set_arguments(&self, args: Value);

    /// Hold this call's service-side invocation open while its current attempt
    /// is abandoned, so a replacement attempt continues the same logical call.
    ///
    /// Returns `false` when there is nothing to hold — the service has not
    /// named the call yet, or this executor has no service behind it — in
    /// which case a restart submits a fresh call instead.
    fn pause_for_restart(&self) -> bool {
        false
    }

    /// Stop this call's current attempt but keep its service-side invocation
    /// open, so the response the Host records for it is what the MCP caller
    /// receives once that response is acknowledged.
    ///
    /// Returns `false` when there is nothing to hold, the service has not named
    /// the call yet, or this executor has no service behind it.
    /// The call is then torn down when its attempt is cancelled.
    fn hold_for_response(&self) -> bool {
        false
    }

    /// Release the call, or answer the question it is waiting on, and return
    /// the next thing it needs from the Host.
    ///
    /// An MCP-backed executor replies on its existing MCP call; the server runs
    /// the tool, or its formatter, again with the answer.
    /// A question can come from either, before approval, after it, or while the
    /// tool runs, and is answered here each time.
    ///
    /// The executor doesn't know how questions should be answered - it just
    /// reports that input is needed.
    /// The coordinator looks up the tool configuration to determine whether to
    /// prompt the user or ask the LLM.
    ///
    /// # Arguments
    ///
    /// - `answers` - Accumulated answers from previous `NeedsInput` responses
    /// - `cancellation_token` - Token to cancel execution
    /// - `stderr` - Receives the tool's stderr lines as they arrive, for a
    ///   caller showing progress while it runs.
    ///   `None` when nothing is watching; the lines still reach tracing and the
    ///   accumulated buffer either way.
    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        cancellation_token: CancellationToken,
        stderr: Option<StderrSink>,
    ) -> ExecutorResult;
}

/// Creates Host-facing tool calls and acknowledges their recorded responses.
pub(crate) trait ExecutorSource: Send + Sync {
    /// The endpoint an external agent submits its own tool calls to.
    ///
    /// `None` when this source has no reachable endpoint, which is every source
    /// that only serves calls JP submits itself.
    fn endpoint(&self) -> Option<Url> {
        None
    }

    /// Choose who submits the MCP request for the calls created after this.
    ///
    /// A source that can only serve calls JP submits refuses anything else,
    /// rather than silently accepting work it will never route.
    fn set_execution(&self, execution: ToolExecution) -> Result<(), ExecutorError> {
        if execution == ToolExecution::Caller {
            return Ok(());
        }
        Err(ExecutorError::ExternalCallsUnsupported)
    }

    /// Release a final delivery barrier after the response has been recorded.
    ///
    /// `review` carries the content the Host settled on, which the executor
    /// compares against what it offered to decide whether the Host edited it.
    fn acknowledge(&self, _review: Review) -> BoxFuture<'_, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    /// Creates an executor for the given tool call request.
    ///
    /// Returns `None` if the tool cannot be resolved (e.g. missing from the
    /// definitions).
    fn create(
        &self,
        request: ToolCallRequest,
        config: ToolConfigWithDefaults,
    ) -> Option<Box<dyn Executor>>;
}

/// What the Host settled on for one call, once the conversation has it.
///
/// [`edited`] is what distinguishes a Host that rewrote the text from one that
/// passed it through: only the executor that offered the original knows which
/// happened, so the comparison is made where the original still exists rather
/// than by projecting both to text and comparing strings.
///
/// [`edited`]: Self::edited
#[derive(Debug, Clone)]
pub(crate) struct Review {
    /// The response the conversation recorded.
    pub response: ToolCallResponse,

    /// Whether the Host changed the content it was offered.
    pub edited: bool,
}

impl Review {
    /// The Host recorded the content it was offered.
    pub fn unchanged(response: ToolCallResponse) -> Self {
        Self {
            response,
            edited: false,
        }
    }

    /// The Host recorded content of its own in place of what it was offered.
    pub fn replaced(response: ToolCallResponse) -> Self {
        Self {
            response,
            edited: true,
        }
    }
}

/// Project a tool result into the conversation's text/error format.
///
/// This is the compatibility projection: the conversation stores one string per
/// call plus a failure flag, so ordered content, resources, and structured data
/// are flattened by [`ToolResult::to_text`] and the failure flag becomes `Err`.
pub(crate) fn response(id: impl Into<String>, result: &ToolResult) -> ToolCallResponse {
    let text = result.to_text();
    ToolCallResponse {
        id: id.into(),
        result: if result.is_error() {
            Err(text)
        } else {
            Ok(text)
        },
    }
}

/// What a call needs from the Host after one step.
///
/// Tools may need multiple rounds of execution if they require additional
/// input.
/// This enum allows the executor to return control to the coordinator, which
/// decides how to handle the `NeedsInput` case by looking up the question
/// configuration.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "A turn holds one of these per in-flight call, not a collection of them"
)]
pub(crate) enum ExecutorResult {
    /// Tool completed (success or error), or was settled before it ran.
    ///
    /// The full result stays with the executor, which hands it back unchanged
    /// if the Host records this response without editing it.
    Completed(ToolCallResponse),

    /// The call is waiting for the Host to admit it, with
    /// [`Executor::approve`].
    AwaitingAdmission,

    /// The call is admitted and waiting for the Host to release it, with
    /// [`Executor::execute`].
    AwaitingRelease,

    /// The call failed before the tool was released to run, so nothing ran.
    ///
    /// Distinct from a tool that ran and reported failure: the reason is JP's
    /// own machinery, not the tool's, so it is not content for the model to
    /// reason about.
    Failed(ExecutorError),

    /// The call failed after the tool was released to run, before its result
    /// arrived.
    ///
    /// The tool may have run to completion, including its side effects, so
    /// running it again is not known to be safe.
    OutcomeUnknown(ExecutorError),

    /// Tool needs additional input before it can continue.
    ///
    /// The executor doesn't know who should answer - it just reports that input
    /// is needed.
    /// The coordinator looks up the question configuration to determine the
    /// target:
    ///
    /// - `User`: Prompt the user interactively, then restart the tool
    /// - `Assistant`: Format a response asking the LLM to re-run with answers
    NeedsInput {
        /// The question that needs to be answered.
        question: Question,

        /// Resolved provenance for the persisted `InquiryRequest`.
        source: InquirySource,

        /// Accumulated answers so far (for retry).
        accumulated_answers: IndexMap<String, Value>,
    },
}

/// Information needed to prompt for tool execution permission.
///
/// This struct contains all the data the `ToolPrompter` needs to show a
/// permission prompt to the user.
#[derive(Debug, Clone)]
pub(crate) struct PermissionInfo {
    /// The tool call ID.
    pub tool_id: String,

    /// The tool name.
    pub tool_name: String,

    /// The tool source (builtin, local, MCP).
    pub tool_source: ToolSource,

    /// The configured run mode.
    pub run_mode: RunMode,

    /// The arguments to pass to the tool.
    pub arguments: Value,
}

#[cfg(test)]
#[path = "executor_mock.rs"]
pub(crate) mod mock;
