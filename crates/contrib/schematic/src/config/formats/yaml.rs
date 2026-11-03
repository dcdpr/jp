use std::path::Path;

use serde::de::DeserializeOwned;

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
        // Anchors, aliases, and merge keys (`<<: *anchor`) are resolved by the
        // deserializer itself, so the document maps straight onto `T`.
        //
        // A failure has to leave the closure as an `Err`: on `Ok`, the caller
        // checks that the input was fully consumed, and a document abandoned
        // mid-way is reported as "multiple YAML documents" instead of the
        // actual error. The path travels out on the side.
        let mut path = String::new();
        let result = serde_saphyr::with_deserializer_from_str(content, |de| {
            serde_path_to_error::deserialize::<_, T>(de).map_err(|error| {
                path = error.path().to_string();
                error.into_inner()
            })
        })
        .map_err(|error| ParserError {
            path,
            message: error.without_snippet().to_string(),
        })?;

        Ok(result)
    }
}
