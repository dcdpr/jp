use std::path::Path;

use serde::de::DeserializeOwned;

use crate::config::{
    error::{ConfigError, HandlerError},
    parser::ParserError,
    source::{Source, SourceFormat},
};

#[derive(Default)]
pub struct TomlFormat {}

impl<T: DeserializeOwned> SourceFormat<T> for TomlFormat {
    fn should_parse(&self, source: &Source) -> bool {
        source.get_file_ext() == Some("toml")
    }

    fn parse(
        &self,
        _source: &Source,
        content: &str,
        _cache_path: Option<&Path>,
    ) -> Result<T, ConfigError> {
        let de = toml::Deserializer::parse(content).map_err(|error| {
            ConfigError::Handler(Box::new(HandlerError::new(error.to_string())))
        })?;

        let result: T = serde_path_to_error::deserialize(de).map_err(|error| ParserError {
            path: error.path().to_string(),
            message: error.inner().message().to_owned(),
        })?;

        Ok(result)
    }
}
