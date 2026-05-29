use std::{
    borrow::Cow,
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;
use tracing::{instrument, trace};

use super::{
    configs::{Config, PartialConfig},
    error::ConfigError,
    layer::Layer,
    source::{Source, SourceFormat},
};
use crate::helpers::strip_bom;

/// The result of loading a configuration.
/// Includes the final configuration, and all layers that were loaded.
#[derive(Serialize)]
pub struct ConfigLoadResult<T: Config> {
    /// Final configuration, after all layers are merged.
    pub config: T,

    /// Partial layers, in order of declaration and extension.
    pub layers: Vec<Layer<T>>,
}

/// A system for loading configuration from multiple sources in multiple
/// formats, and generating a final result after merging and validating layers.
pub struct ConfigLoader<T: Config> {
    _config: PhantomData<T>,
    formats: Vec<Arc<dyn SourceFormat<T::Partial>>>,
    name: String,
    sources: Vec<Source>,
    root: Option<PathBuf>,
}

impl<T: Config> Default for ConfigLoader<T> {
    fn default() -> Self {
        ConfigLoader {
            _config: PhantomData,
            formats: vec![],
            name: T::schema_name().unwrap_or_else(|| "<unknown>".into()),
            sources: vec![],
            root: None,
        }
    }
}

impl<T: Config> ConfigLoader<T> {
    /// Create a new config loader and auto-register formats based on enabled
    /// features.
    #[must_use]
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut loader = ConfigLoader::default();

        #[cfg(feature = "json")]
        loader.add_format(super::formats::JsonFormat::default());

        #[cfg(feature = "toml")]
        loader.add_format(super::formats::TomlFormat::default());

        #[cfg(feature = "yaml")]
        loader.add_format(super::formats::YamlFormat::default());

        loader
    }

    /// Add a format to parse sources based on file name/extension detection.
    pub fn add_format(&mut self, format: impl SourceFormat<T::Partial> + 'static) -> &mut Self {
        self.formats.push(Arc::new(format));
        self
    }

    /// Add a code snippet source to load, with a required file name.
    pub fn code<S: TryInto<String>, P: TryInto<PathBuf>>(
        &mut self,
        code: S,
        path: P,
    ) -> Result<&mut Self, ConfigError> {
        self.source(Source::code(code, path)?)
    }

    /// Add a file source to load.
    pub fn file<P: TryInto<PathBuf>>(&mut self, path: P) -> Result<&mut Self, ConfigError> {
        self.source(Source::file(path, true)?)
    }

    /// Add a file source to load but don't error if the file doesn't exist.
    pub fn file_optional<S: TryInto<PathBuf>>(
        &mut self,
        path: S,
    ) -> Result<&mut Self, ConfigError> {
        self.source(Source::file(path, false)?)
    }

    /// Add a custom source.
    pub fn source(&mut self, source: Source) -> Result<&mut Self, ConfigError> {
        self.sources.push(source);

        Ok(self)
    }

    /// Load, parse, and merge all sources into a final configuration.
    pub fn load(&self) -> Result<ConfigLoadResult<T>, ConfigError> {
        let context = <T::Partial as PartialConfig>::Context::default();

        self.load_with_context(&context)
    }

    /// Load, parse, and merge all sources into a final configuration with the
    /// provided context.
    /// Context will be passed to all applicable default and merge functions
    /// defined with `#[setting]`.
    #[instrument(name = "load_config", skip_all)]
    pub fn load_with_context(
        &self,
        context: &<T::Partial as PartialConfig>::Context,
    ) -> Result<ConfigLoadResult<T>, ConfigError> {
        trace!(config = &self.name, "Loading configuration");

        let layers = self.parse_into_layers(&self.sources, context)?;
        let partial = self.merge_layers(&layers, context)?.finalize(context)?;

        Ok(ConfigLoadResult {
            config: T::from_partial(partial, vec![])?,
            layers,
        })
    }

    /// Load, parse, and merge all sources into a partial configuration with the
    /// provided context.
    /// This does not inherit default values or environment variables.
    ///
    /// Partials can be converted to full with [`Config::from_partial`].
    #[instrument(name = "load_partial_config", skip_all)]
    pub fn load_partial(
        &self,
        context: &<T::Partial as PartialConfig>::Context,
    ) -> Result<T::Partial, ConfigError> {
        trace!(config = &self.name, "Loading partial configuration");

        let layers = self.parse_into_layers(&self.sources, context)?;
        let partial = self.merge_layers(&layers, context)?;

        Ok(partial)
    }

    /// Set the project root directory, for use within error messages.
    pub fn set_root<P: AsRef<Path>>(&mut self, root: P) -> &mut Self {
        self.root = Some(root.as_ref().to_path_buf());
        self
    }

    fn get_location<'l>(&'l self, source: &'l Source) -> &'l str {
        match source {
            Source::Code { .. } => &self.name,
            Source::File { path, .. } => {
                let rel_path = if let Some(root) = &self.root {
                    path.strip_prefix(root).unwrap_or(path)
                } else {
                    path
                };

                rel_path.to_str().unwrap_or(&self.name)
            }
        }
    }

    #[instrument(skip_all)]
    fn merge_layers(
        &self,
        layers: &[Layer<T>],
        context: &<T::Partial as PartialConfig>::Context,
    ) -> Result<T::Partial, ConfigError> {
        trace!(
            config = &self.name,
            "Merging partial layers into a final result"
        );

        // All `None` by default
        let mut merged = T::Partial::default();

        // Then apply other layers in order
        for layer in layers {
            merged.merge(context, layer.partial.clone())?;
        }

        Ok(merged)
    }

    #[instrument(skip_all)]
    fn parse_into_layers(
        &self,
        sources_to_parse: &[Source],
        _context: &<T::Partial as PartialConfig>::Context,
    ) -> Result<Vec<Layer<T>>, ConfigError> {
        let mut layers: Vec<Layer<T>> = vec![];

        for source in sources_to_parse {
            trace!(
                config = &self.name,
                source = source.as_str(),
                "Creating layer from source"
            );

            // Parse the source into a partial
            let partial: T::Partial = self
                .parse_source(source)
                .map_err(|error| self.map_parser_error(error, source))?;

            layers.push(Layer {
                partial,
                source: source.clone(),
            });
        }

        Ok(layers)
    }

    #[instrument(skip_all)]
    fn parse_source(&self, source: &Source) -> Result<T::Partial, ConfigError> {
        let (content, cache_path): (Cow<'_, str>, Option<PathBuf>) = match source {
            Source::Code { code, .. } => (Cow::Borrowed(strip_bom(code)), None),
            Source::File { path, required } => {
                let content = if path.exists() {
                    fs::read_to_string(path).map_err(|error| ConfigError::ReadFileFailed {
                        path: path.clone(),
                        error: Box::new(error),
                    })?
                } else {
                    if *required {
                        return Err(ConfigError::MissingFile(path.clone()));
                    }

                    return Ok(T::Partial::default());
                };

                (Cow::Owned(strip_bom(&content).to_owned()), None)
            }
        };

        for format in &self.formats {
            if format.should_parse(source) {
                return format.parse(source, &content, cache_path.as_deref());
            }
        }

        Err(ConfigError::NoMatchingFormat {
            src: source.as_str().to_owned(),
            ext: source.get_file_ext().unwrap_or("(none)").into(),
        })
    }

    fn map_parser_error(&self, outer: ConfigError, source: &Source) -> ConfigError {
        match outer {
            ConfigError::Parser { error, .. } => ConfigError::Parser {
                location: self.get_location(source).to_owned(),
                error,
            },
            _ => outer,
        }
    }
}
