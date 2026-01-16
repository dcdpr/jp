//! Configuration utilities.

use std::path::{Path, PathBuf};

use glob::glob;
use indexmap::IndexMap;
use schematic::{ConfigLoader, MergeError, MergeResult, PartialConfig, TransformResult};
use tracing::{debug, error, info, trace, warn};

use super::Config;
use crate::{
    AppConfig, BoxedError, PartialAppConfig, error::Error,
    types::extending_path::ExtendingRelativePath,
};

/// Valid file extensions for configuration files.
const VALID_CONFIG_FILE_EXTS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

/// Load multiple partial configurations, starting with the first. Later
/// partials override earlier ones, until one of the partials disables
/// inheritance.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn load_partials_with_inheritance(
    partials: Vec<PartialAppConfig>,
) -> Result<PartialAppConfig, Error> {
    // Start with an empty partial.
    let mut partial = PartialAppConfig::empty();

    // Apply all partials in reverse order (most general to most specific),
    // until we find a partial that has `inherit = false`.
    for p in partials {
        if partial.inherit.is_some_and(|v| !v) {
            break;
        }

        partial.merge(&(), p)?;
    }

    Ok(partial)
}

/// Load environment variables into a partial configuration.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn load_envs(base: PartialAppConfig) -> Result<PartialAppConfig, BoxedError> {
    trace!("Loading environment variable configuration.");
    let mut partial = PartialAppConfig::from_envs()?;
    partial.merge(&(), base)?;

    Ok(partial)
}

/// Tries to find a configuration file in a load path.
pub fn find_file_in_load_path(
    segment: &dyn AsRef<Path>,
    load_path: &dyn AsRef<Path>,
) -> Option<PathBuf> {
    let segment = segment.as_ref();
    let load_path = load_path.as_ref();

    // Segment has to be relative to a load path.
    if segment.has_root() {
        return None;
    }

    let path = load_path.join(segment);

    // If the segment matches a file, return the path as-is.
    if path.is_file() {
        return Some(path);
    }

    // Try and find the file in the load path, trying all valid extensions.
    for ext in VALID_CONFIG_FILE_EXTS {
        let path = path.with_extension(ext);
        if !path.is_file() {
            continue;
        }

        info!(path = %path.display(), "Found configuration file in load path.");
        return Some(path);
    }

    None
}

/// Load a partial configuration from a file at `path`, if it exists.
///
/// This loads either the file directly, or tries to load a file with the same
/// name, but the extension replaced with one of the valid
/// `VALID_CONFIG_FILE_EXTS`.
///
/// # Errors
///
/// See `load_config_file_at_path`.
pub fn load_partial_at_path<P: Into<PathBuf>>(path: P) -> Result<Option<PartialAppConfig>, Error> {
    let mut loader = ConfigLoader::<AppConfig>::new();
    match load_config_file_at_path(path, &mut loader, false) {
        Ok(()) => {}
        Err(Error::Schematic(schematic::ConfigError::MissingFile(_))) => return Ok(None),
        Err(error) => return Err(error),
    }

    loader.load_partial(&()).map(Some).map_err(Into::into)
}

/// Load a partial configuration from a file at `path`, recursing upwards until
/// either `/` or `root` is reached.
///
/// # Errors
///
/// Can error if file parsing fails, or if partial validation fails.
pub fn load_partial_at_path_recursive<P: Into<PathBuf>>(
    path: P,
    root: Option<&Path>,
) -> Result<Option<PartialAppConfig>, Error> {
    let path: PathBuf = path.into();

    // Try and load the provided path as a partial.
    // e.g. `/foo/bar/config.toml`
    let partial = load_partial_at_path(&path)?;

    // Take the file name of the provided path.
    // e.g. `config.toml`
    let mut iter = path.iter();
    let Some(file_name) = iter.next_back() else {
        return Ok(partial);
    };

    // Check if `/foo/bar` is the same as the provided root, if it is, we're done.
    if root.is_some_and(|root| root == iter.as_path()) {
        return Ok(partial);
    }

    // Remove the path segment *before* the file name.
    // e.g. `bar`, or return early if there are no more path segments.
    if iter.next_back().is_none() {
        return Ok(partial);
    }

    // Try and load `/foo/config.toml`
    let path = iter.as_path().join(file_name);
    let fallback = load_partial_at_path_recursive(path, root)?;

    match (partial, fallback) {
        // If we found both `/foo/bar/config.toml` AND `/foo/config.toml`, merge
        // them (longer path taking precedence).
        (Some(partial), Some(mut fallback)) => {
            fallback.merge(&(), partial)?;
            Ok(Some(fallback))
        }

        // otherwise, return either one, or none.
        (partial, fallback) => Ok(partial.or(fallback)),
    }
}

/// Build a final configuration from merged partial configurations.
///
/// # Errors
///
/// Can error if partial validation fails.
pub fn build(mut partial: PartialAppConfig) -> Result<AppConfig, Error> {
    if let Some(mut defaults) = PartialAppConfig::default_values(&())? {
        // The `config` partial is merged into `defaults`. This ensures that,
        // even if a value is `Some` by default, it can be overridden by the
        // explicitly set config value.
        defaults.merge(&(), partial)?;
        partial = defaults;
    }

    debug!("Loading configuration.");
    trace!(
        config = serde_json::to_string(&partial).unwrap_or_default(),
        "Configuration details."
    );

    let mut config: AppConfig = Config::from_partial(partial, vec![])?;

    // Sort instructions by position.
    config
        .assistant
        .instructions
        .sort_by(|a, b| a.position.cmp(&b.position));

    Ok(config)
}

/// Open a configuration file at `path`, if it exists.
///
/// If the file does not exist, the same file name is used but with one of the
/// valid `VALID_CONFIG_FILE_EXTS` extensions.
///
/// # Errors
///
/// Can error if file parsing fails, or if partial validation fails.
fn load_config_file_at_path<P: Into<PathBuf>>(
    path: P,
    loader: &mut ConfigLoader<AppConfig>,
    optional: bool,
) -> Result<(), Error> {
    let mut path: PathBuf = path.into();

    trace!(path = %path.display(), "Trying to open configuration file.");
    if path.is_file() {
        info!(path = %path.display(), "Found configuration file.");
        return load_config_file_with_extends(&path, loader, optional);
    }

    for ext in VALID_CONFIG_FILE_EXTS {
        path.set_extension(ext);
        if !path.is_file() {
            continue;
        }

        info!(path = %path.display(), "Found configuration file.");
        return load_config_file_with_extends(&path, loader, optional);
    }

    Err(Error::Schematic(schematic::ConfigError::MissingFile(path)))
}

/// Load a configuration file at `path`, assuming it exists.
///
/// If the file configures `extends`, those will be loaded as well.
fn load_config_file_with_extends(
    path: &Path,
    loader: &mut ConfigLoader<AppConfig>,
    optional: bool,
) -> Result<(), Error> {
    let root = path.parent().map(Path::to_path_buf);

    let (before, after): (Vec<_>, Vec<_>) = ConfigLoader::<AppConfig>::new()
        .file(path)?
        .load_partial(&())?
        .extends
        .into_iter()
        .flatten()
        .partition(ExtendingRelativePath::is_before);

    load_optional_paths(before, root.as_deref(), loader)?;

    if optional {
        loader.file_optional(path)?;
    } else {
        loader.file(path)?;
    }

    load_optional_paths(after, root.as_deref(), loader)?;

    Ok(())
}

/// Load the optional paths.
fn load_optional_paths(
    extends: impl IntoIterator<Item = ExtendingRelativePath>,
    root: Option<&Path>,
    loader: &mut ConfigLoader<AppConfig>,
) -> Result<(), Error> {
    for path in extends {
        let Some(root) = &root else {
            continue;
        };

        let path = path.to_logical_path(root);
        let Some(path_str) = path.as_os_str().to_str() else {
            continue;
        };

        // Path without glob patterns, warn if it is not a file.
        if !path_str.contains(['*', '?', '[']) && !path.is_file() {
            warn!(path = %path.display(), "Unable to extend with non-existing file");
            continue;
        }

        for entry in glob(path_str)? {
            let path = match entry {
                Ok(path) => path,
                Err(error) => {
                    error!(path = %path.display(), error = %error, "Unable to read glob pattern");
                    continue;
                }
            };

            load_config_file_at_path(&path, loader, true)?;
        }
    }

    Ok(())
}

/// Deduplicate a vector using `transform = vec_dedup`.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
pub(crate) fn vec_dedup<T: PartialEq + Ord>(mut v: Vec<T>, _: &()) -> TransformResult<Vec<T>> {
    v.sort();
    v.dedup();
    Ok(v)
}

/// Merge [`IndexMap`]s of nested [`PartialConfig`]s.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn merge_nested_indexmap<V, C>(
    prev: IndexMap<String, V>,
    mut next: IndexMap<String, V>,
    c: &C,
) -> MergeResult<IndexMap<String, V>>
where
    V: PartialConfig<Context = C>,
    C: Default,
{
    let mut prev = prev
        .into_iter()
        .map(|(name, mut prev)| {
            if let Some(next) = next.shift_remove(&name) {
                prev.merge(c, next).map_err(MergeError::new)?;
            }

            Ok((name, prev))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;

    prev.append(&mut next);
    Ok(Some(prev))
}

/// Define the name to serialize and deserialize for a unit variant.
#[macro_export]
macro_rules! named_unit_variant {
    ($variant:ident) => {
        $crate::named_unit_variant!(stringify!($variant), $variant);
    };
    ($variant:expr, $mod:ident) => {
        pub mod $mod {
            pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str($variant)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<(), D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = ();

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(concat!("\"", $variant, "\""))
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        if value == $variant {
                            Ok(())
                        } else {
                            Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
                        }
                    }
                }

                deserializer.deserialize_str(V)
            }
        }
    };
}

#[cfg(test)]
pub(crate) struct EnvVarGuard {
    name: String,
    original_value: Option<String>,
}

#[cfg(test)]
impl EnvVarGuard {
    pub fn set(name: &str, value: &str) -> Self {
        let name = name.to_string();
        let original_value = std::env::var(&name).ok();
        unsafe { std::env::set_var(&name, value) };
        Self {
            name,
            original_value,
        }
    }
}

#[cfg(test)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(ref original) = self.original_value {
            unsafe { std::env::set_var(&self.name, original) };
        } else {
            unsafe { std::env::remove_var(&self.name) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use assert_matches::assert_matches;
    use serde_json::{Value, json};
    use serial_test::serial;
    use tempfile::tempdir;
    use test_log::test;

    use super::*;
    use crate::{
        assistant::instructions::PartialInstructionsConfig,
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, ProviderId},
        types::vec::{MergedVec, MergedVecStrategy},
    };

    // Helper to write config content to a file, creating parent dirs
    fn write_config(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_load_partials_with_inheritance() {
        struct TestCase {
            partials: Vec<PartialAppConfig>,
            want: (&'static str, Option<Value>),
        }

        let cases = vec![
            ("disabled inheritance", TestCase {
                partials: vec![
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("FOO".to_owned());
                        partial
                    },
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("BAR".to_owned());
                        partial.inherit = Some(false);
                        partial
                    },
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("BAZ".to_owned());
                        partial
                    },
                ],
                want: ("/providers/llm/openrouter/api_key_env", Some("BAR".into())),
            }),
            ("inheritance", TestCase {
                partials: vec![
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("FOO".to_owned());
                        partial
                    },
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("BAR".to_owned());
                        partial.inherit = Some(true);
                        partial
                    },
                    {
                        let mut partial = PartialAppConfig::empty();
                        partial.providers.llm.openrouter.api_key_env = Some("BAZ".to_owned());
                        partial
                    },
                ],
                want: ("/providers/llm/openrouter/api_key_env", Some("BAZ".into())),
            }),
        ];

        for (name, case) in cases {
            let partial = load_partials_with_inheritance(case.partials).unwrap();
            let json = serde_json::to_value(&partial).unwrap();
            let val = json.pointer(case.want.0);

            assert_eq!(val, case.want.1.as_ref(), "failed case: {name}");
        }
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_envs() {
        let _env = EnvVarGuard::set("JP_CFG_PROVIDERS_LLM_OPENROUTER_API_KEY_ENV", "ENV1");

        let partial = load_envs(PartialAppConfig::empty()).unwrap();
        assert_eq!(
            partial.providers.llm.openrouter.api_key_env,
            Some("ENV1".to_owned())
        );
    }

    #[test]
    fn test_build() {
        let error = build(PartialAppConfig::default_values(&()).unwrap().unwrap()).unwrap_err();
        assert_matches!(
            error,
            Error::Schematic(schematic::ConfigError::MissingRequired { .. })
        );

        let mut partial = PartialAppConfig::default_values(&()).unwrap().unwrap();
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Openrouter),
            name: Some("foo".parse().unwrap()),
        }
        .into();

        partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

        let config = build(partial).unwrap();
        assert_eq!(
            config.providers.llm.openrouter.api_key_env,
            "OPENROUTER_API_KEY".to_owned()
        );
    }

    #[test]
    fn test_build_without_required_fields() {
        use schematic::ConfigError::MissingRequired;

        let mut partial = PartialAppConfig::default_values(&()).unwrap().unwrap();

        let error = build(partial.clone()).unwrap_err();
        assert_matches!(error, Error::Schematic(MissingRequired { fields }) if fields == vec!["assistant", "model", "id", "provider"]);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Openrouter),
            name: Some("foo".parse().unwrap()),
        }
        .into();

        let error = build(partial.clone()).unwrap_err();
        assert_matches!(error, Error::Schematic(MissingRequired{ fields }) if fields == vec!["conversation", "tools", "defaults", "run"]);
        partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

        build(partial).unwrap();
    }

    #[test]
    fn test_build_sorted_instructions() {
        let mut partial = PartialAppConfig::empty();
        partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Openrouter),
            name: Some("foo".parse().unwrap()),
        }
        .into();
        partial.assistant.instructions = MergedVec {
            value: vec![
                PartialInstructionsConfig {
                    title: None,
                    description: None,
                    position: Some(100),
                    items: Some(vec![]),
                    examples: vec![],
                },
                PartialInstructionsConfig {
                    title: None,
                    description: None,
                    position: Some(-1),
                    items: Some(vec![]),
                    examples: vec![],
                },
                PartialInstructionsConfig {
                    title: None,
                    description: None,
                    position: Some(0),
                    items: Some(vec![]),
                    examples: vec![],
                },
            ],
            strategy: MergedVecStrategy::Replace,
            is_default: false,
        }
        .into();

        let config = build(partial).unwrap();

        assert_eq!(config.assistant.instructions[0].position, -1);
        assert_eq!(config.assistant.instructions[1].position, 0);
        assert_eq!(config.assistant.instructions[2].position, 100);
    }

    #[test]
    fn test_load_partial_at_path() {
        struct TestCase {
            file: &'static str,
            data: &'static str,
            arg: &'static str,
            want: Result<Option<&'static str>, &'static str>,
        }

        let cases = vec![
            ("exact match toml", TestCase {
                file: "config.toml",
                data: "providers.llm.openrouter.api_key_env = 'FOO'",
                arg: "config.toml",
                want: Ok(Some("FOO")),
            }),
            ("exact match json", TestCase {
                file: "config.json",
                data: r#"{"providers":{"llm":{"openrouter":{"api_key_env":"FOO"}}}}"#,
                arg: "config.json",
                want: Ok(Some("FOO")),
            }),
            ("exact match yaml", TestCase {
                file: "config.yaml",
                data: "providers:\n  llm:\n    openrouter:\n      api_key_env: FOO",
                arg: "config.yaml",
                want: Ok(Some("FOO")),
            }),
            ("toml mismatch", TestCase {
                file: "config.toml",
                data: "providers.llm.openrouter.api_key_env = 'FOO'",
                arg: "config.json",
                want: Ok(Some("FOO")),
            }),
            ("json mismatch", TestCase {
                file: "config.json",
                data: r#"{"providers":{"llm":{"openrouter":{"api_key_env":"FOO"}}}}"#,
                arg: "config.yaml",
                want: Ok(Some("FOO")),
            }),
            ("yaml mismatch", TestCase {
                file: "config.yaml",
                data: "providers:\n  llm:\n    openrouter:\n      api_key_env: FOO",
                arg: "config.toml",
                want: Ok(Some("FOO")),
            }),
            ("no extension", TestCase {
                file: "config.toml",
                data: "providers.llm.openrouter.api_key_env = 'FOO'",
                arg: "config",
                want: Ok(Some("FOO")),
            }),
            ("no match", TestCase {
                file: "config.ini",
                data: "",
                arg: "config.toml",
                want: Ok(None),
            }),
            ("found invalid file", TestCase {
                file: "config.ini",
                data: "",
                arg: "config.ini",
                want: Err("Unsupported format for"),
            }),
        ];

        for (name, case) in cases {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            write_config(&root.join(case.file), case.data);

            let partial = load_partial_at_path(root.join(case.arg));
            if let Err(err) = &case.want {
                assert!(partial.is_err(), "failed case: {name}");
                let actual = partial.unwrap_err().to_string();
                assert!(
                    actual.starts_with(err),
                    "failed case: {name}, expected error '{actual}' to start with '{err}'"
                );
                continue;
            }

            assert_eq!(
                partial
                    .map(|r| r.and_then(|p| p.providers.llm.openrouter.api_key_env))
                    .map_err(|e| e.to_string()),
                case.want
                    .map(|v| v.map(str::to_owned))
                    .map_err(str::to_owned),
                "failed case: {name}",
            );
        }
    }

    #[test]
    fn test_load_partial_at_path_recursive() {
        struct TestCase {
            files: Vec<(&'static str, &'static str)>,
            path: &'static str,
            root: Option<&'static str>,
            want: Result<Option<(&'static str, Option<Value>)>, &'static str>,
        }

        let cases = vec![
            ("override from longest path", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "providers.llm.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"providers":{"llm":{"openrouter":{"api_key_env":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/config.toml",
                root: None,
                want: Ok(Some((
                    "/providers/llm/openrouter/api_key_env",
                    Some("FOO".into()),
                ))),
            }),
            ("merge different paths", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "providers.llm.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/config.toml",
                root: None,
                want: Ok(Some((
                    "/providers/llm/openrouter",
                    Some(json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
                ))),
            }),
            ("find upstream", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "providers.llm.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/bar/baz/config.yaml",
                root: None,
                want: Ok(Some((
                    "/providers/llm/openrouter",
                    Some(json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
                ))),
            }),
            ("merge until root", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "providers.llm.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/bar/config.yaml",
                root: Some("foo"),
                want: Ok(Some((
                    "/providers/llm/openrouter",
                    Some(json!({"api_key_env": "FOO"})),
                ))),
            }),
            ("load dir instead of file", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "providers.llm.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo",
                root: None,
                want: Ok(None),
            }),
            ("regular extends with string replace", TestCase {
                files: vec![
                    (
                        // loaded first, merged last
                        "config.toml",
                        indoc::indoc!(
                            r#"
                            extends = ["one.toml", "two.toml"]
                            assistant.system_prompt = "foo"
                        "#
                        ),
                    ),
                    (
                        // loaded second, merged first
                        "one.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = "bar"
                        "#
                        ),
                    ),
                    (
                        // loaded third, merged second
                        "two.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = "baz"
                        "#
                        ),
                    ),
                ],
                path: "config.toml",
                root: None,
                want: Ok(Some(("/assistant/system_prompt", Some("foo".into())))),
            }),
            ("regular extends with merged string", TestCase {
                files: vec![
                    (
                        // loaded first, merged last
                        "config.toml",
                        indoc::indoc!(
                            r#"
                            extends = ["one.toml", "two.toml"]
                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                        ),
                    ),
                    (
                        // loaded second, merged first
                        "one.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = "baz"
                        "#
                        ),
                    ),
                    (
                        // loaded third, merged second
                        "two.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "bar", strategy = "prepend" }
                        "#
                        ),
                    ),
                ],
                path: "config.toml",
                root: None,
                want: Ok(Some((
                    "/assistant/system_prompt",
                    Some(json!({ "value": "foobarbaz", "strategy": "prepend" })),
                ))),
            }),
            ("nested extends with merged string", TestCase {
                files: vec![
                    (
                        // loaded first, merged last
                        "config.toml",
                        indoc::indoc!(
                            r#"
                            extends = ["one.toml", "three.toml"]
                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                        ),
                    ),
                    (
                        // loaded second, merged second
                        "one.toml",
                        indoc::indoc!(
                            r#"
                            extends = [{ path = "two.toml", strategy = "after" }]
                            assistant.system_prompt = "baz"
                        "#
                        ),
                    ),
                    (
                        // loaded third, merged first
                        "two.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "qux", strategy = "append" }
                        "#
                        ),
                    ),
                    (
                        // loaded fourth, merged third
                        "three.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "bar", strategy = "prepend" }
                        "#
                        ),
                    ),
                ],
                path: "config.toml",
                root: None,
                want: Ok(Some((
                    "/assistant/system_prompt",
                    Some(json!({ "value": "foobarbazqux", "strategy": "prepend" })),
                ))),
            }),
            ("complex extends", TestCase {
                files: vec![
                    (
                        // loaded first, merged fourth
                        "config.toml",
                        indoc::indoc!(
                            r#"
                            extends = [
                                "one.toml",
                                { path = "two.toml", strategy = "before" },
                                { path = "three.toml", strategy = "after" },
                            ]

                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                        ),
                    ),
                    (
                        // loaded second, merged second
                        "one.toml",
                        indoc::indoc!(
                            r#"
                            extends = [{ path = "four.toml", strategy = "before" }]

                            assistant.system_prompt = { value = "bar", strategy = "append" }
                        "#
                        ),
                    ),
                    (
                        // loaded fourth, merged third
                        "two.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "baz", strategy = "append" }
                        "#
                        ),
                    ),
                    (
                        // loaded fifth, merged last
                        "three.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "qux", strategy = "append" }
                        "#
                        ),
                    ),
                    (
                        // loaded third, merged first
                        "four.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "quux", strategy = "replace" }
                        "#
                        ),
                    ),
                    (
                        // ignored
                        "five.toml",
                        indoc::indoc!(
                            r#"
                            assistant.system_prompt = { value = "ignored", strategy = "replace" }
                        "#
                        ),
                    ),
                ],
                path: "config.toml",
                root: None,
                want: Ok(Some((
                    "/assistant/system_prompt",
                    Some(json!({"value": "fooquuxbarbazqux", "strategy": "append"})),
                ))),
            }),
        ];

        for (name, case) in cases {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            for (file, data) in case.files {
                write_config(&root.join(file), data);
            }
            let root_arg = case.root.map(|r| root.join(r));

            let got = load_partial_at_path_recursive(root.join(case.path), root_arg.as_deref());

            match (got, case.want) {
                (Err(got), Err(want)) => assert_eq!(got.to_string(), want.to_owned()),
                (Ok(None), Ok(None)) => {}
                (Ok(Some(got)), Ok(Some((path, want)))) => {
                    let json = serde_json::to_value(&got).unwrap();
                    let val = json.pointer(path);
                    assert_eq!(val, want.as_ref(), "failed case: {name}");
                }
                (got, want) => {
                    panic!("failed case: {name}\n\ngot:  {got:?}\nwant: {want:?}")
                }
            }
        }
    }
}
