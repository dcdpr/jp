/// A failure in the tool domain: resolving a tool, reading its parameter
/// schema, checking the arguments a call carries, or running the tool itself.
///
/// A tool that ran and reported a problem of its own is not an `Error`: that is
/// [`Outcome::Error`], which the caller hands back to the model.
///
/// [`Outcome::Error`]: crate::Outcome::Error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A recognized tool-result envelope is malformed.
    #[error("Malformed tool output: {0}")]
    MalformedOutput(#[source] serde_json::Error),

    #[error("Tool not found: {name}")]
    NotFound { name: String },

    #[error("Tools not found: {}", names.join(", "))]
    NotFoundN { names: Vec<String> },

    #[error("Command missing for local tool")]
    MissingCommand,

    /// Wraps the MCP client's own error, which this crate does not name so it
    /// stays independent of the MCP implementation.
    #[error("Failed to fetch tool from MCP client")]
    McpGetToolError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Wraps the MCP client's own error, which this crate does not name so it
    /// stays independent of the MCP implementation.
    #[error("Failed to run tool from MCP client")]
    McpRunToolError(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Failed to spawn command: {command}")]
    SpawnError {
        command: String,
        #[source]
        error: std::io::Error,
    },

    /// `data` is the template that failed to render, kept for the diagnostic.
    #[error("Template error")]
    TemplateError {
        data: String,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Invalid schema at `{path}`: {message}")]
    InvalidSchema { path: String, message: String },

    #[error("Invalid arguments (missing: {missing:?}, unknown: {unknown:?})")]
    Arguments {
        /// Required arguments that were missing.
        missing: Vec<String>,

        /// Unknown arguments that were provided.
        unknown: Vec<String>,
    },
}

#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}
