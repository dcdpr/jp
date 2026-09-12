//! Failures while the MCP Host advances a logical tool call.

use std::error::Error as StdError;

use serde_json::Error as JsonError;
use tokio::task::JoinError;

/// A tool-call adapter failed before completing its Host protocol.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Execution was requested before submitting a call.
    #[error("MCP call has not started")]
    NotStarted,
    /// A call was submitted more than once.
    #[error("MCP call was prepared twice")]
    AlreadyPrepared,
    /// The service lost its Host interaction channel.
    #[error("MCP Host interaction channel closed")]
    HostDisconnected,
    /// Result metadata could not be decoded into the shared result contract.
    #[error("Invalid tool result: {0}")]
    MalformedResult(#[source] JsonError),
    /// The transport task terminated unexpectedly.
    #[error(transparent)]
    Task(#[from] JoinError),
    /// An MCP request failed.
    #[error("{0}")]
    Transport(#[source] Box<dyn StdError + Send + Sync>),
    /// Approval is not the next operation for this call.
    #[error("MCP call is not awaiting approval")]
    NotAwaitingApproval,
    /// A prepared call stopped before the Host replied.
    #[error("MCP approval expired")]
    ApprovalExpired,
    /// The call stopped before execution was released.
    #[error("MCP release expired")]
    ReleaseExpired,
    /// The call stopped before an input answer was supplied.
    #[error("MCP inquiry expired")]
    InquiryExpired,
    /// No answer exists for the outstanding question.
    #[error("Missing answer to pending MCP inquiry")]
    MissingAnswer,
    /// The service rejected execution with a tool diagnostic.
    #[error("{message}")]
    Rejected { message: String },
    /// The call was cancelled by the Host.
    #[error("Tool execution cancelled.")]
    Cancelled,
    /// The service produced an interaction outside the recording protocol.
    #[error("Unexpected MCP interaction during recording")]
    UnexpectedRecording,
    /// The service produced an interaction outside the preparation protocol.
    #[error("Unexpected MCP preparation interaction")]
    UnexpectedPreparation,
    /// Approval did not reach a release barrier.
    #[error("MCP call did not reach the release barrier")]
    MissingRelease,
    /// The service produced an interaction outside the execution protocol.
    #[error("Unexpected MCP execution interaction")]
    UnexpectedExecution,
    /// The result approved by the service differs from recorded content.
    #[error("MCP result differs from the recorded response")]
    RecordingMismatch,
    /// The delivered response differs from recorded content.
    #[error("MCP response differs from the recorded response")]
    DeliveryMismatch,
    /// The legacy inquiry interface accepts only textual choices.
    #[error("Non-string inquiry choice")]
    NonStringChoice,
    /// The legacy inquiry interface cannot present this schema.
    #[error("Unsupported tool inquiry schema")]
    UnsupportedInquirySchema,
}
