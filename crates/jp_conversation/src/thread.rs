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
    /// Each section is rendered via [`SectionConfig::render()`] before
    /// being sent to the provider.
    pub sections: Vec<SectionConfig>,

    /// The attachments to use.
    pub attachments: Vec<Attachment>,

    /// The conversation events to use.
    pub events: ConversationStream,
}

/// A rendered piece of system content, tagged by origin.
///
/// Providers that need to apply per-part cache control (e.g. Anthropic,
/// OpenRouter) can match on the variant to decide cache placement. Providers
/// that don't care can call [`into_inner()`](SystemPart::into_inner) to get the
/// plain string.
#[derive(Debug, Clone)]
pub enum SystemPart {
    /// A prompt or section string (system prompt, instructions, context).
    Prompt(String),

    /// Rendered attachment XML.
    Attachment(String),
}

impl SystemPart {
    /// Consume the part and return the inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        match self {
            Self::Prompt(s) | Self::Attachment(s) => s,
        }
    }

    /// Returns whether the part is a prompt.
    #[must_use]
    pub const fn is_prompt(&self) -> bool {
        matches!(self, Self::Prompt(_))
    }

    /// Returns whether the part is an attachment.
    #[must_use]
    pub const fn is_attachment(&self) -> bool {
        matches!(self, Self::Attachment(_))
    }
}

/// The decomposed parts of a [`Thread`], ready for provider consumption.
///
/// System content (prompt, sections, attachments) is rendered to tagged
/// [`SystemPart`]s. Conversation events are filtered to exclude internal types
/// that providers should never see.
pub struct ThreadParts {
    /// Rendered system content, tagged by origin.
    pub system_parts: Vec<SystemPart>,

    /// Conversation events filtered to provider-visible events only.
    pub events: ConversationStream,
}

impl Thread {
    /// Decompose the thread into rendered system parts and filtered events.
    ///
    /// System prompt, sections, and attachments are rendered to strings. Events
    /// are filtered via `EventKind::is_provider_visible()` to exclude internal
    /// types.
    ///
    /// # Errors
    ///
    /// Returns an error if attachment XML serialization fails.
    pub fn into_parts(self) -> Result<ThreadParts> {
        let Self {
            system_prompt,
            sections,
            attachments,
            mut events,
        } = self;

        let mut system_parts = vec![];

        if let Some(system_prompt) = system_prompt {
            system_parts.push(SystemPart::Prompt(system_prompt));
        }

        for section in &sections {
            system_parts.push(SystemPart::Prompt(section.render()));
        }

        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            system_parts.push(SystemPart::Attachment(documents.try_to_xml()?));
        }

        events.retain(|e| e.kind.is_provider_visible());

        Ok(ThreadParts {
            system_parts,
            events,
        })
    }

    /// Convert the thread into a list of messages.
    ///
    /// Delegates to [`into_parts()`](Self::into_parts) for system content
    /// rendering and event filtering, then applies the provider-specific
    /// conversion closures.
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
        let parts = self.into_parts()?;
        let strings: Vec<String> = parts
            .system_parts
            .into_iter()
            .map(SystemPart::into_inner)
            .collect();

        let mut items = vec![];
        items.extend(to_system_messages(strings));
        items.extend(convert_stream(parts.events));

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
