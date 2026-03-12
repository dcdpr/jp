pub mod db;
pub mod error;
pub mod fts;
pub mod note;
pub mod schema;
pub mod search;
pub mod server;
pub mod tag;

pub use db::BearDb;
pub use error::Error;
pub use note::Note;
pub use search::{SearchMatch, SearchMode, SearchParams};
pub use tag::Tag;

type Result<T> = std::result::Result<T, Error>;
