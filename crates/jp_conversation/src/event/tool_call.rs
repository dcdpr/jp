//! See [`ToolCallRequest`] and [`ToolCallResponse`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// A tool call request event - requesting execution of a tool.
///
/// This event is typically triggered by the assistant as part of its response,
/// but can also be triggered automatically by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Unique identifier for this tool call
    pub id: String,

    /// Name of the tool to execute
    pub name: String,

    /// Arguments to pass to the tool
    #[serde(with = "jp_serde::repr::base64_json_map")]
    pub arguments: Map<String, Value>,
}

impl ToolCallRequest {
    /// Creates a new tool call request.
    #[must_use]
    pub const fn new(id: String, name: String, arguments: Map<String, Value>) -> Self {
        Self {
            id,
            name,
            arguments,
        }
    }
}

/// A tool call response event - the result of executing a tool.
///
/// This event MUST be in response to a `ToolCallRequest` event, with a matching
/// `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallResponse {
    /// ID matching the corresponding `ToolCallRequest`
    pub id: String,

    /// The result of executing the tool: `Ok(content)` on success, `Err(error)`
    /// on failure
    pub result: Result<String, String>,
}

impl ToolCallResponse {
    /// Get the content of the response, either the result or the error.
    #[must_use]
    pub fn content(&self) -> &str {
        match &self.result {
            Ok(content) | Err(content) => content,
        }
    }

    /// Consume the response and get the content, either the result or the
    /// error.
    #[must_use]
    pub fn into_content(self) -> String {
        match self.result {
            Ok(content) | Err(content) => content,
        }
    }
}

// Custom serialization to maintain backward compatibility with the JSON format
impl Serialize for ToolCallResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[allow(clippy::allow_attributes, clippy::missing_docs_in_private_items)]
        struct Helper<'a> {
            id: &'a str,
            #[serde(with = "jp_serde::repr::base64_string")]
            content: &'a str,
            is_error: bool,
        }

        let (content, is_error) = match &self.result {
            Ok(content) => (content, false),
            Err(content) => (content, true),
        };

        Helper {
            id: &self.id,
            content,
            is_error,
        }
        .serialize(serializer)
    }
}

// Custom deserialization to maintain backward compatibility with the JSON format
impl<'de> Deserialize<'de> for ToolCallResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[allow(clippy::allow_attributes, clippy::missing_docs_in_private_items)]
        struct Helper {
            id: String,
            #[serde(with = "jp_serde::repr::base64_string")]
            content: String,
            is_error: bool,
        }

        let helper = Helper::deserialize(deserializer)?;

        Ok(Self {
            id: helper.id,
            result: if helper.is_error {
                Err(helper.content)
            } else {
                Ok(helper.content)
            },
        })
    }
}
