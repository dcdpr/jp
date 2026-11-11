//! Persistent mapping from each chat to its active conversation for the current
//! rollover period (daily or weekly).
//!
//! Each chat uses at most one conversation per period so persona changes take
//! effect at the next rollover and conversation context stays bounded.
//! Each entry records the start date of its period.
//! The mapping is stored in `state.json` in the user data directory, outside
//! the repository checkout.

use std::{collections::BTreeMap, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::transport::Chat;

const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    date: NaiveDate,
    conversation: String,
}

/// Map from a chat's key (its `Display` form) to its latest conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Chats {
    chats: BTreeMap<String, Entry>,
}

/// Persistent chat-to-conversation state (`path = None` keeps state in memory).
#[derive(Debug)]
pub(crate) struct State {
    path: Option<Utf8PathBuf>,
    chats: Chats,
}

impl State {
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            chats: Chats::default(),
        }
    }

    /// Load `state.json` from `dir`, creating `dir` if needed.
    ///
    /// Returns an empty [`State`] if `state.json` does not exist yet.
    pub(crate) fn open(dir: &Utf8Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|error| format!("failed to create {dir}: {error}"))?;
        let path = dir.join(STATE_FILE);

        let chats = match fs::read_to_string(&path) {
            Ok(source) => serde_json::from_str(&source)
                .map_err(|error| format!("{path} is not the bridge's state: {error}"))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Chats::default(),
            Err(error) => return Err(format!("failed to read {path}: {error}")),
        };

        Ok(Self {
            path: Some(path),
            chats,
        })
    }

    /// Return the conversation ID recorded for `chat` for the period starting
    /// on `period`, if any.
    pub(crate) fn conversation(&self, chat: &Chat, period: NaiveDate) -> Option<&str> {
        self.chats
            .chats
            .get(&chat.to_string())
            .filter(|entry| entry.date == period)
            .map(|entry| entry.conversation.as_str())
    }

    /// Record `conversation` as `chat`'s active conversation for the period
    /// starting on `period`.
    pub(crate) fn set(
        &mut self,
        chat: &Chat,
        period: NaiveDate,
        conversation: &str,
    ) -> Result<(), String> {
        self.chats.chats.insert(chat.to_string(), Entry {
            date: period,
            conversation: conversation.to_owned(),
        });

        self.save()
    }

    /// Forget `chat`'s conversation, so its next turn starts a new one.
    ///
    /// The conversation itself is kept, and expires as usual.
    pub(crate) fn forget(&mut self, chat: &Chat) -> Result<(), String> {
        if self.chats.chats.remove(&chat.to_string()).is_none() {
            return Ok(());
        }

        self.save()
    }

    /// Write the mapping atomically, via a temporary file and a rename.
    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };

        let json = serde_json::to_string_pretty(&self.chats)
            .map_err(|error| format!("failed to serialize the bridge's state: {error}"))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, format!("{json}\n"))
            .and_then(|()| fs::rename(&temporary, path))
            .map_err(|error| format!("failed to write {path}: {error}"))
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
