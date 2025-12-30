//! See [`Thread`].

use jp_attachment::Attachment;
use jp_config::assistant::instructions::InstructionsConfig;
use quick_xml::se::TextFormat;
use serde::Serialize;
use tracing::trace;

use crate::{
    ConversationStream,
    error::{Error, Result},
};

/// A builder for creating a Thread with events.
#[derive(Debug, Clone, Default)]
pub struct ThreadBuilder {
    /// See [`Thread::system_prompt`].
    pub system_prompt: Option<String>,

    /// See [`Thread::instructions`].
    pub instructions: Vec<InstructionsConfig>,

    /// See [`Thread::attachments`].
    pub attachments: Vec<Attachment>,

    /// See [`Thread::events`].
    pub events: Option<ConversationStream>,
}

impl ThreadBuilder {
    /// Creates a new builder with the given initial configuration.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            system_prompt: None,
            instructions: Vec::new(),
            attachments: Vec::new(),
            events: None,
        }
    }

    /// Set the system prompt for the thread.
    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    /// Set the instructions for the thread.
    #[must_use]
    pub fn with_instructions(mut self, instructions: Vec<InstructionsConfig>) -> Self {
        self.instructions = instructions;
        self
    }

    /// Add an instruction to the thread.
    #[must_use]
    pub fn add_instruction(mut self, instruction: InstructionsConfig) -> Self {
        self.instructions.push(instruction);
        self
    }

    /// Set the attachments for the thread.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Set the events for the thread.
    #[must_use]
    pub fn with_events(mut self, events: ConversationStream) -> Self {
        self.events = Some(events);
        self
    }

    // /// Add an event to the thread.
    // ///
    // /// If no [`ConversationStream`] exists, a default one is created.
    // #[must_use]
    // pub fn with_event(mut self, event: impl Into<ConversationEvent>) -> Self {
    //     let mut stream = self.events.take().unwrap_or_default();
    //     stream.push(event);
    //     self.with_events(stream)
    // }

    /// Build the thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the thread is missing any required fields.
    pub fn build(self) -> Result<Thread> {
        let Self {
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

/// A collection of details that describe the contents of a conversation.
///
/// This type is passed to the LLM providers to generate an HTTP request that
/// contains all the information needed to generate a response.
#[derive(Debug, Clone)]
pub struct Thread {
    /// The system prompt to use.
    pub system_prompt: Option<String>,

    /// The instructions to use.
    pub instructions: Vec<InstructionsConfig>,

    /// The attachments to use.
    pub attachments: Vec<Attachment>,

    /// The conversation events to use.
    pub events: ConversationStream,
}

impl Thread {
    /// Convert the thread into a list of messages.
    ///
    /// # Errors
    ///
    /// Returns an error if XML serialization fails.
    pub fn into_messages<T, U, M, S>(
        self,
        to_system_messages: M,
        convert_stream: S,
    ) -> Result<Vec<T>>
    where
        U: IntoIterator<Item = T>,
        M: Fn(Vec<String>) -> U,
        S: Fn(ConversationStream) -> Vec<T>,
    {
        let Self {
            system_prompt,
            instructions,
            attachments,
            events,
        } = self;

        let mut items = vec![];
        let mut parts = vec![];

        if let Some(system_prompt) = system_prompt {
            parts.push(system_prompt);
        }

        for instruction in &instructions {
            parts.push(instruction.try_to_xml()?);
        }

        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            parts.push(documents.try_to_xml()?);
        }

        items.extend(to_system_messages(parts));
        items.extend(convert_stream(events));

        Ok(items)
    }
}

/// Structure for document collection
///
/// See: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
#[derive(Debug, Serialize)]
#[serde(rename = "documents", rename_all = "camelCase")]
pub struct Documents {
    /// The documents to include in the XML.
    #[serde(rename = "document")]
    documents: Vec<Document>,
}

impl Documents {
    /// Generate XML from Documents struct.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn try_to_xml(&self) -> Result<String> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);

        if self
            .documents
            .iter()
            .any(|v| v.content.contains(['<', '>']))
        {
            serializer.text_format(TextFormat::CData);
        }

        self.serialize(serializer)?;
        Ok(buffer)
    }
}

/// XML structure for a single document
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    /// The index of the document in the list of documents.
    index: usize,

    /// The source of the document.
    source: String,

    /// The content of the document.
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
