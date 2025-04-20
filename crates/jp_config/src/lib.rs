mod config;
pub mod error;
pub mod llm;
mod parse;
pub mod style;

pub use config::Config;
pub use error::Error;
pub use parse::load;

fn set_error(s: impl Into<String>) -> error::Result<()> {
    Err(Error::UnknownConfigKey {
        key: s.into(),
        available_keys: Config::fields(),
    })
}
