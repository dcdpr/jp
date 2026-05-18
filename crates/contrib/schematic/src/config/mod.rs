mod configs;
mod error;
mod formats;
mod layer;
mod loader;
mod merger;
mod parser;
mod path;
mod source;

pub use configs::*;
pub use error::*;
#[cfg(feature = "json")]
pub use formats::JsonFormat;
#[cfg(feature = "toml")]
pub use formats::TomlFormat;
#[cfg(feature = "yaml")]
pub use formats::YamlFormat;
pub use layer::*;
pub use loader::*;
pub use merger::*;
pub use parser::*;
pub use path::*;
pub use source::*;

#[macro_export]
macro_rules! derive_enum {
    ($impl:item) => {
        #[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
        #[serde(rename_all = "kebab-case")]
        $impl
    };
}

pub type DefaultValueResult<T> = std::result::Result<Option<T>, HandlerError>;
pub type TransformResult<T> = std::result::Result<T, HandlerError>;

#[cfg(feature = "env")]
pub type ParseEnvResult<T> = std::result::Result<Option<T>, HandlerError>;
