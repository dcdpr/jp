use std::fmt;

use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_term::table::{DetailItem, DetailRow, details};

use super::datetime::DateTimeFmt;

pub struct DetailsFmt {
    /// The ID of the conversation.
    pub id: ConversationId,

    /// The name of the assistant, if any.
    pub assistant_name: Option<String>,

    /// The conversation title.
    pub title: Option<String>,

    /// The number of events in the conversation.
    pub message_count: usize,

    /// The number of turns in the conversation.
    pub turn_count: usize,

    /// Whether the conversation is pinned.
    /// If `None`, the details are not shown.
    pub pinned: Option<bool>,

    /// Whether the conversation is local.
    /// If `None`, the details are not shown.
    pub local: Option<bool>,

    /// Mark the active conversation.
    pub active_conversation: Option<ConversationId>,

    /// Display the timestamp of the last message in the conversation.
    pub last_message_at: Option<DateTime<Utc>>,

    /// Display the last time the conversation was activated.
    pub last_activated_at: Option<DateTime<Utc>>,

    /// Display the timestamp of conversation expiration.
    pub expires_at: Option<DateTime<Utc>>,

    /// Attachments associated with the conversation.
    pub attachments: Vec<DetailItem>,

    /// Pretty-print the output.
    pub pretty: bool,
}

impl DetailsFmt {
    #[must_use]
    pub fn new(id: ConversationId) -> Self {
        Self {
            id,
            assistant_name: None,
            title: None,
            message_count: 0,
            turn_count: 0,
            pinned: None,
            local: None,
            active_conversation: None,
            last_message_at: None,
            last_activated_at: None,
            expires_at: None,
            attachments: vec![],
            pretty: true,
        }
    }

    #[must_use]
    pub fn with_event_count(mut self, message_count: usize) -> Self {
        self.message_count = message_count;
        self
    }

    #[must_use]
    pub fn with_turn_count(mut self, turn_count: usize) -> Self {
        self.turn_count = turn_count;
        self
    }

    #[must_use]
    pub fn with_last_message_at(mut self, last_message_at: Option<DateTime<Utc>>) -> Self {
        self.last_message_at = last_message_at;
        self
    }

    #[must_use]
    pub fn with_last_activated_at(mut self, last_activated_at: Option<DateTime<Utc>>) -> Self {
        self.last_activated_at = last_activated_at;
        self
    }

    #[must_use]
    pub fn with_expires_at(mut self, expires_at: Option<DateTime<Utc>>) -> Self {
        self.expires_at = expires_at;
        self
    }

    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<DetailItem>) -> Self {
        self.attachments = attachments;
        self
    }

    #[must_use]
    pub fn with_title(mut self, title: Option<impl Into<String>>) -> Self {
        self.title = title.map(Into::into);
        self
    }

    #[must_use]
    pub fn with_pinned_flag(self, pinned: bool) -> Self {
        Self {
            pinned: Some(pinned),
            ..self
        }
    }

    #[must_use]
    pub fn with_local_flag(self, local: bool) -> Self {
        Self {
            local: Some(local),
            ..self
        }
    }

    /// Mark the active conversation.
    #[must_use]
    pub fn with_active_conversation(self, active_conversation: ConversationId) -> Self {
        Self {
            active_conversation: Some(active_conversation),
            ..self
        }
    }

    /// Use color in the output.
    #[must_use]
    pub fn with_pretty_printing(self, pretty: bool) -> Self {
        Self { pretty, ..self }
    }

    /// Return the title of the conversation.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Return rows for a table displaying the conversation details.
    #[must_use]
    pub fn rows(&self) -> Vec<DetailRow> {
        let mut rows = vec![];

        rows.push(self.scalar("ID", self.id.to_string()));
        if let Some(name) = self.assistant_name.clone() {
            rows.push(self.scalar("Assistant", name));
        }

        if self.message_count > 0 {
            rows.push(self.scalar("Events", self.message_count.to_string()));
        }

        if self.turn_count > 0 {
            rows.push(self.scalar("Turns", self.turn_count.to_string()));
        }

        if let Some(last_message_at) = self.last_message_at {
            rows.push(self.scalar(
                "Latest Message",
                DateTimeFmt::new(last_message_at).to_string(),
            ));
        }

        if let Some(active) = self.active_conversation {
            let value = if active == self.id && self.pretty {
                "Currently Active".green().bold().to_string()
            } else if active == self.id {
                "Currently Active".to_owned()
            } else if let Some(last_activated_at) = self.last_activated_at {
                DateTimeFmt::new(last_activated_at).to_string()
            } else {
                "Unknown".to_owned()
            };
            rows.push(self.scalar("Last Activated", value));
        }

        if let Some(expires_at) = self.expires_at {
            let value = if expires_at < Utc::now() {
                "On Deactivation".to_string()
            } else {
                DateTimeFmt::new(expires_at).to_string()
            };
            rows.push(self.scalar("Expires In", value));
        }

        if let Some(pinned) = self.pinned {
            let value = if pinned {
                "Yes".bold().blue().to_string()
            } else {
                "No".to_string()
            };
            rows.push(self.scalar("Pinned", value));
        }

        if let Some(local) = self.local {
            let value = if local {
                "Yes".bold().yellow().to_string()
            } else {
                "No".to_string()
            };
            rows.push(self.scalar("Local", value));
        }

        if !self.attachments.is_empty() {
            rows.push(DetailRow::list(
                self.styled_label("Attachments"),
                self.attachments.clone(),
            ));
        }

        rows
    }

    /// Bold the label when pretty-printing is enabled.
    fn styled_label(&self, label: &str) -> String {
        if self.pretty {
            label.bold().to_string()
        } else {
            label.to_owned()
        }
    }

    fn scalar(&self, label: &str, value: String) -> DetailRow {
        DetailRow::scalar(self.styled_label(label), value)
    }
}

impl fmt::Display for DetailsFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", details(self.title(), self.rows()))
    }
}
