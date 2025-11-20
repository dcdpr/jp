use jp_attachment::Attachment;
use jp_config::assistant::instructions::InstructionsConfig;
use serde::Serialize;

use crate::{
    ConversationStream,
    error::{Error, Result},
};

/// A builder for creating a Thread with events.
#[derive(Debug, Clone, Default)]
pub struct ThreadBuilder {
    pub system_prompt: Option<String>,
    pub instructions: Vec<InstructionsConfig>,
    pub attachments: Vec<Attachment>,
    pub events: Option<ConversationStream>,
}

impl ThreadBuilder {
    /// Creates a new builder with the given initial configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            instructions: Vec::new(),
            attachments: Vec::new(),
            events: None,
        }
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: Vec<InstructionsConfig>) -> Self {
        self.instructions = instructions;
        self
    }

    #[must_use]
    pub fn add_instruction(mut self, instruction: InstructionsConfig) -> Self {
        self.instructions.push(instruction);
        self
    }

    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments = attachments;
        self
    }

    #[must_use]
    pub fn with_events(mut self, events: ConversationStream) -> Self {
        self.events = Some(events);
        self
    }

    pub fn build(self) -> Result<Thread> {
        let ThreadBuilder {
            system_prompt,
            instructions,
            attachments,
            events,
        } = self;

        let events =
            events.ok_or_else(|| Error::Thread("Event stream not initialized".to_string()))?;

        Ok(Thread {
            system_prompt,
            instructions,
            attachments,
            events,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub system_prompt: Option<String>,
    pub instructions: Vec<InstructionsConfig>,
    pub attachments: Vec<Attachment>,
    pub events: ConversationStream,
}

/// Structure for document collection
///
/// See: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
#[derive(Debug, Serialize)]
#[serde(rename = "documents", rename_all = "camelCase")]
pub struct Documents {
    #[serde(rename = "document")]
    documents: Vec<Document>,
}

impl Documents {
    /// Generate XML from Documents struct.
    pub fn try_to_xml(&self) -> Result<String> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }
}

/// XML structure for a single document
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    index: usize,
    source: String,
    content: String,
}

impl From<(usize, Attachment)> for Document {
    fn from((index, attachment): (usize, Attachment)) -> Self {
        Self {
            index,
            source: attachment.source,
            content: attachment.content,
        }
    }
}

impl From<Vec<Document>> for Documents {
    fn from(documents: Vec<Document>) -> Self {
        Self { documents }
    }
}
