use jp_attachment::Attachment;
use serde::Serialize;

use crate::{
    error::{Error, Result},
    persona::Instructions,
    Conversation, MessagePair, Model, UserMessage,
};

/// A wrapper for multiple messages, with convenience methods for adding
/// specific message types and content.
#[derive(Debug, Default, Clone)]
pub struct ThreadBuilder {
    pub conversation: Conversation,
    pub model: Option<Model>,
    pub system_prompt: Option<String>,
    pub instructions: Vec<Instructions>,
    pub attachments: Vec<Attachment>,
    pub history: Vec<MessagePair>,
    pub reasoning: Option<String>,
    pub message: Option<UserMessage>,
}

impl ThreadBuilder {
    #[must_use]
    pub fn new(conversation: Conversation) -> Self {
        Self {
            conversation,
            model: None,
            system_prompt: None,
            instructions: vec![],
            attachments: vec![],
            history: vec![],
            reasoning: None,
            message: None,
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: Model) -> Self {
        self.model = Some(model);
        self
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: Vec<Instructions>) -> Self {
        self.instructions.extend(instructions);
        self
    }

    #[must_use]
    pub fn with_instruction(mut self, instruction: Instructions) -> Self {
        self.instructions.push(instruction);
        self
    }

    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments.extend(attachments);
        self
    }

    #[must_use]
    pub fn with_history(mut self, history: Vec<MessagePair>) -> Self {
        self.history.extend(history);
        self
    }

    #[must_use]
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<UserMessage>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn build(self) -> Result<Thread> {
        let ThreadBuilder {
            conversation,
            model,
            system_prompt,
            instructions,
            attachments,
            history,
            reasoning,
            message,
        } = self;

        let model = model.ok_or(Error::Thread("Missing model".to_string()))?;
        let message = message.ok_or(Error::Thread("Missing message".to_string()))?;

        Ok(Thread {
            conversation,
            model,
            system_prompt,
            instructions,
            attachments,
            history,
            reasoning,
            message,
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct Thread {
    pub conversation: Conversation,
    pub model: Model,
    pub system_prompt: Option<String>,
    pub instructions: Vec<Instructions>,
    pub attachments: Vec<Attachment>,
    pub history: Vec<MessagePair>,
    pub reasoning: Option<String>,
    pub message: UserMessage,
}

impl Thread {
    #[must_use]
    pub fn with_message(mut self, message: UserMessage) -> Self {
        self.message = message;
        self
    }
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

#[derive(Debug, Serialize)]
pub struct Thinking(pub String);

impl Thinking {
    pub fn try_to_xml(&self) -> Result<String> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }
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
