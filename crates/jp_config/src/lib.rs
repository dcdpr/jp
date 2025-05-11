mod config;
pub mod conversation;
pub mod error;
pub mod llm;
mod parse;
pub mod style;

pub use config::Config;
pub use error::Error;
pub use parse::{build, load, load_envs, load_partial, PartialConfig};

fn set_error(s: impl Into<String>) -> error::Result<()> {
    Err(Error::UnknownConfigKey {
        key: s.into(),
        available_keys: Config::fields(),
    })
}
