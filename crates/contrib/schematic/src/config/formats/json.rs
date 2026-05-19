use std::path::Path;

use serde::de::DeserializeOwned;

use crate::config::{
    error::ConfigError,
    parser::ParserError,
    source::{Source, SourceFormat},
};

#[derive(Default)]
pub struct JsonFormat {}

impl<T: DeserializeOwned> SourceFormat<T> for JsonFormat {
    fn should_parse(&self, source: &Source) -> bool {
        source
            .get_file_ext()
            .is_some_and(|ext| ext == "json" || ext == "jsonc")
    }

    fn parse(
        &self,
        source: &Source,
        content: &str,
        _cache_path: Option<&Path>,
    ) -> Result<T, ConfigError> {
        let mut content = String::from(if content.is_empty() { "{}" } else { content });

        json_strip_comments::strip(&mut content).map_err(|error| {
            ConfigError::JsonStripCommentsFailed {
                file: source.get_file_name().to_owned(),
                error: Box::new(error),
            }
        })?;

        let de = &mut serde_json::Deserializer::from_str(&content);

        let result: T = serde_path_to_error::deserialize(de).map_err(|error| ParserError {
            path: error.path().to_string(),
            message: error.inner().to_string(),
        })?;

        Ok(result)
    }
}
