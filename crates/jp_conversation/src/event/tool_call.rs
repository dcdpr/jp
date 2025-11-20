use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A tool call request event - requesting execution of a tool.
///
/// This event is typically triggered by the assistant as part of its response,
/// but can also be triggered automatically by the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Unique identifier for this tool call
    pub id: String,

    /// Name of the tool to execute
    pub name: String,

    /// Arguments to pass to the tool
    #[serde(with = "jp_serde::repr::base64_json_map")]
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

impl ToolCallRequest {
    #[must_use]
    pub fn new(
        id: String,
        name: String,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            name,
            arguments,
        }
    }
}

/// A tool call response event - the result of executing a tool.
///
/// This event MUST be in response to a `ToolCallRequest` event, with a matching `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallResponse {
    /// ID matching the corresponding `ToolCallRequest`
    pub id: String,

    /// The result of executing the tool: `Ok(content)` on success, `Err(error)` on failure
    pub result: Result<String, String>,
}

impl ToolCallResponse {
    #[must_use]
    pub fn content(&self) -> &str {
        match &self.result {
            Ok(content) | Err(content) => content,
        }
    }

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
