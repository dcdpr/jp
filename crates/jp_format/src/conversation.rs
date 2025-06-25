use std::fmt;

use comfy_table::{Cell, CellAlignment, Row, Table};
use crossterm::style::Stylize as _;
use jp_conversation::{Conversation, ConversationId, MessagePair};
use time::UtcDateTime;

use crate::datetime::DateTimeFmt;

pub struct DetailsFmt {
    /// The ID of the conversation.
    pub id: ConversationId,

    /// The name of the assistant, if any.
    pub assistant_name: Option<String>,

    /// The conversation title.
    pub title: Option<String>,

    /// The number of messages in the conversation.
    pub message_count: usize,

    /// Whether the conversation is local. If `None`, the details are not shown.
    pub local: Option<bool>,

    /// Mark the active conversation.
    pub active_conversation: Option<ConversationId>,

    /// Display the timestamp of the last message in the conversation.
    pub last_message_at: Option<UtcDateTime>,

    /// Display the last time the conversation was activated.
    pub last_activated_at: UtcDateTime,

    /// Use OSC-8 hyperlinks.
    pub hyperlinks: bool,

    /// Use color in the output.
    pub color: bool,
}

impl DetailsFmt {
    #[must_use]
    pub fn new(id: ConversationId, conversation: Conversation, messages: &[MessagePair]) -> Self {
        let Conversation {
            title,
            last_activated_at,
            ..
        } = conversation;

        let last_message_at = messages.iter().map(|m| m.timestamp).max();

        Self {
            id,
            assistant_name: conversation.config.assistant.name.clone(),
            title: title.clone(),
            message_count: messages.len(),
            local: None,
            active_conversation: None,
            last_message_at,
            last_activated_at,
            hyperlinks: true,
            color: true,
        }
    }

    #[must_use]
    pub fn with_title(self, title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
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
    pub fn with_color(self, color: bool) -> Self {
        Self { color, ..self }
    }

    /// Use OSC-8 hyperlinks.
    #[must_use]
    pub fn with_hyperlinks(self, hyperlinks: bool) -> Self {
        Self { hyperlinks, ..self }
    }

    /// Return the title of the conversation.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Return rows for a table displaying the conversation details.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut map = vec![];

        map.push(("ID".to_owned(), self.id.to_string()));
        if let Some(name) = self.assistant_name.clone() {
            map.push(("Assistant".to_owned(), name));
        }

        if let Some(last_message_at) = self.last_message_at {
            map.push((
                "Latest Message".to_owned(),
                DateTimeFmt::new(last_message_at).to_string(),
            ));
        }

        if let Some(active) = self.active_conversation {
            map.push((
                "Last Activated".to_owned(),
                if active == self.id && self.color {
                    "Currently Active".green().bold().to_string()
                } else if active == self.id {
                    "Currently Active".to_owned()
                } else {
                    DateTimeFmt::new(self.last_activated_at).to_string()
                },
            ));
        }

        if let Some(local) = self.local {
            map.push((
                "Local".to_owned(),
                if local {
                    "Yes".bold().yellow().to_string()
                } else {
                    "No".to_string()
                },
            ));
        }

        let mut rows = vec![];
        for (key, value) in map {
            let mut row = Row::new();
            row.add_cell(
                Cell::new(if self.color {
                    key.bold().to_string()
                } else {
                    key
                })
                .set_alignment(CellAlignment::Right),
            );
            row.add_cell(Cell::new(value).set_alignment(CellAlignment::Left));
            rows.push(row);
        }

        rows
    }
}

impl fmt::Display for DetailsFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rows = self.rows();
        let mut buf = String::new();

        if let Some(title) = self.title() {
            buf.push_str(title);
            buf.push_str("\n\n");
        }

        let mut table = Table::new();
        table.load_preset(comfy_table::presets::NOTHING);
        table.add_rows(rows);
        buf.push_str(&table.trim_fmt());

        write!(f, "{buf}")
    }
}
