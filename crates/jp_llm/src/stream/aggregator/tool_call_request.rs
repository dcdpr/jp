use indexmap::IndexMap;
use jp_conversation::event::ToolCallRequest;
use serde_json::{Map, from_str};

/// Accumulates partial data for a tool call request and validates it upon
/// completion.
#[derive(Debug, Clone, Default)]
pub struct ToolCallRequestAggregator {
    pending: IndexMap<usize, ToolCallBuffer>,
}

impl ToolCallRequestAggregator {
    /// Creates a new, empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingests a partial chunk of a tool call request.
    ///
    /// - `id`: First non-empty ID wins.
    /// - `name`: First non-empty Name wins.
    /// - `partial_json`: Appended to the arguments buffer.
    pub fn add_chunk(
        &mut self,
        index: usize,
        id: Option<String>,
        name: Option<String>,
        partial_json: Option<&str>,
    ) {
        self.pending
            .entry(index)
            .or_default()
            .add_chunk(id, name, partial_json);
    }

    /// Finalizes the aggregation, validates the JSON, and produces the
    /// `ToolCallRequest`.
    ///
    /// If successful, the state of the aggregator is reset.
    pub fn finalize(&mut self, index: usize) -> Result<ToolCallRequest, AggregationError> {
        let Some(mut buffer) = self.pending.shift_remove(&index) else {
            return Err(AggregationError::UnknownIndex);
        };

        let Some(id) = buffer.id.take_if(|v| !v.is_empty()) else {
            return Err(AggregationError::MissingId);
        };

        let Some(name) = buffer.name.take_if(|v| !v.is_empty()) else {
            return Err(AggregationError::MissingName);
        };

        let arguments = match buffer.arguments_buffer.as_deref().map(str::trim) {
            Some(buffer) if !buffer.is_empty() => {
                from_str(buffer).map_err(AggregationError::InvalidJson)?
            }
            _ => Map::new(),
        };

        buffer.arguments_buffer = None;

        Ok(ToolCallRequest {
            id,
            name,
            arguments,
        })
    }

    /// Similar to [`Self::finalize`], but finalizes all pending tool call
    /// requests.
    pub fn finalize_all(&mut self) -> IndexMap<usize, Result<ToolCallRequest, AggregationError>> {
        let indices = self.pending.keys().copied().collect::<Vec<_>>();

        indices
            .into_iter()
            .map(|index| (index, self.finalize(index)))
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct ToolCallBuffer {
    id: Option<String>,
    name: Option<String>,
    arguments_buffer: Option<String>,
}

impl ToolCallBuffer {
    /// Ingests a partial chunk of a tool call request.
    ///
    /// - `id`: First non-empty ID wins.
    /// - `name`: First non-empty Name wins.
    /// - `partial_json`: Appended to the arguments buffer.
    pub fn add_chunk(
        &mut self,
        id: Option<String>,
        name: Option<String>,
        partial_json: Option<&str>,
    ) {
        // Handle ID: First Write Wins
        if let Some(id) = id
            && !id.is_empty()
            && self.id.as_ref().is_none_or(String::is_empty)
        {
            self.id = Some(id);
        }

        // Handle Name: First Write Wins
        if let Some(name) = name
            && !name.is_empty()
            && self.name.as_ref().is_none_or(String::is_empty)
        {
            self.name = Some(name);
        }

        // Handle Arguments: Streaming Concatenation
        if let Some(json) = partial_json {
            self.arguments_buffer.get_or_insert_default().push_str(json);
        }
    }
}

/// Errors that can occur when finalizing a partial tool call.
#[derive(Debug, thiserror::Error)]
pub enum AggregationError {
    /// The tool call being finalized was never received.
    #[error("tool call index not found")]
    UnknownIndex,

    /// The tool call buffer finished but no ID was ever received.
    #[error("tool call missing ID")]
    MissingId,
    /// The tool call buffer finished but no Name was ever received.
    #[error("tool call missing name")]
    MissingName,
    /// The accumulated arguments string was not valid JSON.
    #[error("tool call arguments JSON parse error")]
    InvalidJson(#[from] serde_json::Error),
}
