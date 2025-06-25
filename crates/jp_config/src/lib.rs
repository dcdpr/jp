pub mod assignment;
pub mod assistant;
mod config;
pub mod conversation;
pub mod editor;
pub(crate) mod error;
mod map;
pub mod model;
pub mod parse;
pub mod style;
pub mod template;

pub use config::{Config, PartialConfig};
pub use confique::{Config as Configurable, Partial};
pub use error::Error;
pub use parse::{build, find_file_in_path, load_envs, load_partial, load_partial_from_file};

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
}

fn is_empty<T: Partial + PartialEq>(v: &T) -> bool {
    v == &T::empty()
}
