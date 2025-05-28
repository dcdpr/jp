use serde::{Deserialize, Serialize};

use super::{request::RequestMessage, tool::ToolCall};

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Content>,

    #[serde(default, skip_serializing)]
    pub reasoning: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content.push(Content::Text {
            text: text.into(),
            cache_control: None,
        });
        self
    }

    #[must_use]
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    #[must_use]
    pub fn with_content(mut self, content: Vec<Content>) -> Self {
        self.content = content;
        self
    }

    #[must_use]
    pub fn with_cache(mut self) -> Self {
        self.cached();
        self
    }

    pub fn cached(&mut self) {
        if let Some(Content::Text { cache_control, .. }) = self.content.last_mut() {
            *cache_control = Some(CacheControl::Ephemeral);
        }
    }

    #[must_use]
    pub fn system(self) -> RequestMessage {
        RequestMessage::System(self)
    }

    #[must_use]
    pub fn user(self) -> RequestMessage {
        RequestMessage::User(self)
    }

    #[must_use]
    pub fn assistant(self) -> RequestMessage {
        RequestMessage::Assistant(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename = "lowercase", rename_all = "kebab-case")]
pub enum Transform {
    MiddleOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "lowercase", rename_all = "lowercase", tag = "type")]
pub enum Content {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    pub fn disable_cache(&mut self) {
        match self {
            Self::Text { cache_control, .. } => *cache_control = None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename = "snake_case", rename_all = "lowercase", tag = "type")]
pub enum CacheControl {
    Ephemeral,
}
