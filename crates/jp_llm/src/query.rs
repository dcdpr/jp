pub mod chat;
pub mod structured;

pub use chat::ChatQuery;
use jp_conversation::thread::Thread;
pub use structured::StructuredQuery;

#[derive(Debug)]
pub enum Query {
    Chat(ChatQuery),
    Structured(StructuredQuery),
}

impl Query {
    /// Get the [`Thread`] for the query.
    #[must_use]
    pub fn thread(&self) -> &Thread {
        match self {
            Self::Chat(v) => &v.thread,
            Self::Structured(v) => &v.thread,
        }
    }

    /// Get a mutable reference to the [`Thread`] for the query.
    #[must_use]
    pub fn thread_mut(&mut self) -> &mut Thread {
        match self {
            Self::Chat(v) => &mut v.thread,
            Self::Structured(v) => &mut v.thread,
        }
    }

    /// Returns `true` if the query is a chat query.
    #[must_use]
    pub fn is_chat(&self) -> bool {
        matches!(self, Self::Chat(_))
    }

    /// Returns `true` if the query is a structured query.
    #[must_use]
    pub fn is_structured(&self) -> bool {
        matches!(self, Self::Structured(_))
    }
}
