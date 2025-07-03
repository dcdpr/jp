#![feature(const_type_name)]

pub mod assignment;
pub mod assistant;
mod config;
pub mod conversation;
pub mod editor;
pub(crate) mod error;
mod map;
pub mod mcp;
pub mod model;
pub mod parse;
pub(crate) mod serde;
pub mod style;
pub mod template;

pub use config::{Config, PartialConfig};
pub use confique::{Config as Configurable, Partial};
pub use error::Error;
pub use parse::{build, find_file_in_path, load_envs, load_partial, load_partial_from_file};
