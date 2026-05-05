//! See [`Thread`].

use jp_attachment::Attachment;
use jp_config::assistant::sections::SectionConfig;
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

    /// See [`Thread::sections`].
    pub sections: Vec<SectionConfig>,

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
            sections: Vec::new(),
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

    /// Set the sections for the thread.
    #[must_use]
    pub fn with_sections(mut self, sections: Vec<SectionConfig>) -> Self {
        self.sections = sections;
        self
    }

    /// Add a section to the thread.
    #[must_use]
    pub fn add_section(mut self, section: SectionConfig) -> Self {
        self.sections.push(section);
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

    /// Build the thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the thread is missing any required fields.
    pub fn build(self) -> Result<Thread> {
        let Self {
            system_prompt,
            sections,
            attachments,
            events,
        } = self;

        let events =
            events.ok_or_else(|| Error::Thread("Event stream not initialized".to_string()))?;

        Ok(Thread {
            system_prompt,
            sections,
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

    /// The sections to include after the system prompt.
    ///
    /// Each section is rendered via [`SectionConfig::render()`] before being
    /// sent to the provider.
    pub sections: Vec<SectionConfig>,

    /// The attachments to use.
    pub attachments: Vec<Attachment>,

    /// The conversation events to use.
    pub events: ConversationStream,
}

/// The decomposed parts of a [`Thread`], ready for provider consumption.
///
/// System content (prompt, sections) is rendered to strings. Attachments are
/// passed through as-is so that each provider can convert them to its native
/// format. Conversation events are filtered to exclude internal types that
/// providers should never see.
pub struct ThreadParts {
    /// Rendered system content (prompt + sections).
    pub system_parts: Vec<String>,

    /// Raw attachments for providers to handle natively.
    pub attachments: Vec<Attachment>,

    /// Conversation events filtered to provider-visible events only.
    pub events: ConversationStream,
}

impl Thread {
    /// Decompose the thread into rendered system parts, raw attachments, and
    /// filtered events.
    ///
    /// System prompt and sections are rendered to strings. Attachments are
    /// passed through unconverted — each provider is responsible for converting
    /// them to its native format (e.g. Anthropic document blocks, Gemini inline
    /// data, or XML for providers without native support).
    ///
    /// Events are filtered via `EventKind::is_provider_visible()` to exclude
    /// internal types.
    #[must_use]
    pub fn into_parts(self) -> ThreadParts {
        let Self {
            system_prompt,
            sections,
            attachments,
            mut events,
        } = self;

        let mut system_parts = vec![];

        if let Some(system_prompt) = system_prompt {
            system_parts.push(system_prompt);
        }

        for section in &sections {
            system_parts.push(section.render());
        }

        events.apply_projection();
        events.retain(|e| e.kind.is_provider_visible());

        ThreadParts {
            system_parts,
            attachments,
            events,
        }
    }
}

/// Serialize text attachments to an XML `<documents>` block.
///
/// Binary attachments are silently skipped. Callers that support binary
/// content should handle those separately via the raw attachments.
///
/// # Errors
///
/// Returns an error if XML serialization fails.
pub fn text_attachments_to_xml(attachments: &[Attachment]) -> Result<Option<String>> {
    let docs: Vec<Document> = attachments
        .iter()
        .enumerate()
        .filter_map(|(i, attachment)| {
            let text = attachment.as_text()?;

            trace!("Attaching {i}: {}", attachment.source);

            Some(Document {
                index: i,
                source: attachment.source.clone(),
                content: text.to_owned(),
            })
        })
        .collect();

    if docs.is_empty() {
        return Ok(None);
    }

    let documents = Documents { documents: docs };
    documents.try_to_xml().map(Some)
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
