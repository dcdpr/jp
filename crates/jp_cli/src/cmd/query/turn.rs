use indexmap::IndexMap;
use serde_json::Value;

/// State that is persisted for the duration of a turn.
#[derive(Debug, Default)]
pub struct TurnState {
    /// Tool answers that are instructed to be re-used for the duration of the
    /// turn.
    ///
    /// For example, if a tool `foo` asks a question `bar`, and the user
    /// indicates that the same answer should be used during this  turn, then
    /// this map will contain a key `foo` with a value that contains a key
    /// `bar` with the [`Value`] of the answer.
    pub persisted_tool_answers: IndexMap<String, IndexMap<String, Value>>,

    /// The number of times we've tried a request to the assistant.
    ///
    /// This is used when the assistant returns an error that is retryable.
    /// Every retry increments this counter, until a maximum number of retries
    /// is reached.
    pub request_count: usize,
}
