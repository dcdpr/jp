use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::{
    openrouter::{ChatMessage, Role},
    FileArtifact,
};

/// A wrapper for multiple messages, with convenience methods for adding
/// specific message types and content.
#[derive(Debug, Default, Clone)]
pub struct ThreadBuilder {
    system: Option<String>,
    artifacts: Vec<FileArtifact>,
    reasoning: Option<String>,
    history: Vec<ChatMessage>,
    query: Option<String>,
    instructions: Option<String>,
}

impl ThreadBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_artifacts(mut self, artifacts: impl Iterator<Item = FileArtifact>) -> Self {
        self.artifacts.extend(artifacts);
        self
    }

    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    pub fn with_history(mut self, history: Vec<ChatMessage>) -> Self {
        self.history.extend(history);
        self
    }

    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn build(self) -> Result<Vec<ChatMessage>> {
        let Self {
            system,
            artifacts,
            reasoning,
            history,
            query,
            instructions,
        } = self;

        let mut messages = vec![];

        // System message first, if any.
        if let Some(content) = system {
            messages.push(ChatMessage {
                role: Role::System,
                content,
            });
        }

        // Then large list of artifacts, formatted as XML.
        //
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/use-xml-tags>
        if !artifacts.is_empty() {
            let documents: Documents = artifacts
                .into_iter()
                .enumerate()
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            messages.push(ChatMessage {
                role: Role::User,
                content: documents.try_to_xml()?,
            });
        }

        // Then instructions in XML tags.
        if let Some(instructions) = instructions {
            messages.push(ChatMessage {
                role: Role::User,
                content: Instructions(instructions).try_to_xml()?,
            });
        }

        // Historical messages third.
        messages.extend(history);

        // User query
        if let Some(content) = query {
            messages.push(ChatMessage {
                role: Role::User,
                content,
            });
        }

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            messages.push(ChatMessage {
                role: Role::Assistant,
                content: Thinking(content).try_to_xml()?,
            });
        }

        Ok(messages)
    }
}

#[derive(Debug, Serialize)]
pub struct Instructions(pub String);

impl Instructions {
    pub fn try_to_xml(&self) -> Result<String> {
        quick_xml::se::to_string(self)
            .map_err(|err| anyhow!("Failed to serialize Instructions to XML: {}", err))
    }
}

/// Structure for document collection
///
/// See: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
#[derive(Debug, Serialize)]
pub struct Documents {
    #[serde(rename = "document")]
    documents: Vec<Document>,
}

impl Documents {
    /// Generate XML from Documents struct.
    pub fn try_to_xml(&self) -> Result<String> {
        quick_xml::se::to_string(self)
            .map_err(|err| anyhow!("Failed to serialize Documents to XML: {}", err))
    }
}

/// XML structure for a single document
#[derive(Debug, Serialize)]
pub struct Document {
    index: usize,
    source: String,
    document_content: String,
}

#[derive(Debug, Serialize)]
pub struct Thinking(pub String);

impl Thinking {
    pub fn try_to_xml(&self) -> Result<String> {
        quick_xml::se::to_string(self)
            .map_err(|err| anyhow!("Failed to serialize Thinking to XML: {}", err))
    }
}

impl From<(usize, FileArtifact)> for Document {
    fn from((index, artifact): (usize, FileArtifact)) -> Self {
        Self {
            index,
            source: artifact.relative_path.display().to_string(),
            document_content: artifact.content,
        }
    }
}

impl From<Vec<Document>> for Documents {
    fn from(documents: Vec<Document>) -> Self {
        Self { documents }
    }
}
