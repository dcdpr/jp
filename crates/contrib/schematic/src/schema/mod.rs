mod generator;
mod renderer;
mod renderers;

pub use generator::*;
pub use indexmap;
pub use renderer::*;
/// Renders JSON config templates.
#[cfg(all(feature = "renderer_template", feature = "json"))]
pub use renderers::json_template::*;
/// Renders JSONC config templates.
#[cfg(all(feature = "renderer_template", feature = "json"))]
pub use renderers::jsonc_template::*;
/// Helpers for config templates.
#[cfg(feature = "renderer_template")]
pub use renderers::template::TemplateOptions;
/// Renders TOML config templates.
#[cfg(all(feature = "renderer_template", feature = "toml"))]
pub use renderers::toml_template::*;
/// Renders YAML config templates.
#[cfg(all(feature = "renderer_template", feature = "yaml"))]
pub use renderers::yaml_template::*;
pub use schematic_types::*;
