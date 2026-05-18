use std::path::Path;

use serde::de::{DeserializeOwned, IntoDeserializer};
use serde_yaml::{Deserializer, Error, Value};

use crate::config::{
    error::ConfigError,
    parser::ParserError,
    source::{Source, SourceFormat},
};

#[derive(Default)]
pub struct YamlFormat {}

impl<T: DeserializeOwned> SourceFormat<T> for YamlFormat {
    fn should_parse(&self, source: &Source) -> bool {
        source
            .get_file_ext()
            .is_some_and(|ext| ext == "yml" || ext == "yaml")
    }

    fn parse(
        &self,
        _source: &Source,
        content: &str,
        _cache_path: Option<&Path>,
    ) -> Result<T, ConfigError> {
        fn create_parser_error(path: String, error: &Error) -> ParserError {
            ParserError {
                path,
                message: error.to_string(),
            }
        }

        // First pass, convert string to value
        let de = Deserializer::from_str(content);

        let mut result: Value = serde_path_to_error::deserialize(de)
            .map_err(|error| create_parser_error(error.path().to_string(), &error.into_inner()))?;

        // Applies anchors/aliases/references
        result
            .apply_merge()
            .map_err(|error| create_parser_error(String::new(), &error))?;

        // Second pass, convert value to struct
        let de = result.into_deserializer();

        let result: T = serde_path_to_error::deserialize(de)
            .map_err(|error| create_parser_error(error.path().to_string(), &error.into_inner()))?;

        Ok(result)
    }
}
