//! Failures while the MCP Host advances a logical tool call.

use std::error::Error as StdError;

use serde_json::Error as JsonError;
use tokio::task::JoinError;

/// A tool-call adapter failed before completing its Host protocol.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ExecutorError {
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

    /// The call stopped before the Host's reply reached the service.
    ///
    /// `operation` names the barrier that lapsed: `approval`, `release`, or
    /// `inquiry`.
    #[error("MCP {operation} expired")]
    ReplyExpired {
        /// The barrier the Host was answering.
        operation: &'static str,
    },

    /// No answer exists for the outstanding question.
    #[error("Missing answer to pending MCP inquiry")]
    MissingAnswer,

    /// The service rejected execution with a tool diagnostic.
    #[error("{message}")]
    Rejected {
        /// The diagnostic to report in place of a result.
        message: String,
    },

    /// The call was cancelled by the Host.
    #[error("Tool execution cancelled.")]
    Cancelled,

    /// The Host asked for something this call's phase cannot do.
    #[error("MCP call cannot {operation} while {phase}")]
    OutOfOrder {
        /// What the Host asked for.
        operation: &'static str,
        /// The phase the call is in.
        phase: &'static str,
    },

    /// The service asked for an interaction outside the expected sequence.
    ///
    /// Reaching this means the service and this adapter disagree about the
    /// interaction protocol, not that a tool or the user did anything wrong.
    #[error("Unexpected MCP interaction while {phase} a tool call")]
    UnexpectedInteraction {
        /// What the adapter was doing.
        phase: &'static str,
    },

    /// The content the caller received differs from the content recorded.
    #[error("MCP response differs from the recorded response")]
    DeliveryMismatch,

    /// The legacy inquiry interface accepts only textual choices.
    #[error("Non-string inquiry choice")]
    NonStringChoice,

    /// The legacy inquiry interface cannot present this schema.
    #[error("Unsupported tool inquiry schema")]
    UnsupportedInquirySchema,
}
