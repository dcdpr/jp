use std::{
    env,
    path::{Path, PathBuf},
};

use confique::{Config as Confique, File, Partial as _};
use directories::ProjectDirs;
use path_clean::PathClean as _;
use tracing::{debug, info, trace};

use super::Config;
use crate::{Error, PartialConfig};

const APPLICATION: &str = "jp";
const GLOBAL_CONFIG_ENV_VAR: &str = "JP_GLOBAL_CONFIG_FILE";
const VALID_CONFIG_FILE_EXTS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

/// Load multiple partial configurations, starting with the first. Later
/// partials override earlier ones, until one of the partials disables
/// inheritance.
#[must_use]
pub fn load_partials_with_inheritance(partials: Vec<PartialConfig>) -> PartialConfig {
    // Start with an empty partial.
    let mut partial = PartialConfig::empty();

    // Apply all partials in reverse order (most general to most specific),
    // until we find a partial that has `inherit = false`.
    for p in partials {
        if partial.inherit.is_some_and(|v| !v) {
            break;
        }

        partial = load_partial(p, partial);
    }

    partial
}

/// Load environment variables into a partial configuration.
pub fn load_envs(base: PartialConfig) -> Result<PartialConfig, Error> {
    trace!("Loading environment variable configuration.");
    Ok(Config::set_from_envs()?.with_fallback(base))
}

/// Load a partial configuration, with optional fallback.
#[must_use]
pub fn load_partial(partial: PartialConfig, fallback: PartialConfig) -> PartialConfig {
    partial.with_fallback(fallback)
}

pub fn find_file_in_path<S: AsRef<Path>, P: AsRef<Path>>(
    segment: S,
    load_path: P,
) -> Result<Option<PathBuf>, Error> {
    let segment = segment.as_ref();
    let load_path = load_path.as_ref();

    // Segment has to be relative to a load path.
    if segment.has_root() {
        return Ok(None);
    }

    let path = load_path.join(segment);

    // If the segment matches a file, return the path as-is.
    if path.is_file() {
        return Ok(Some(path));
    }

    // Try and find the file in the load path, trying all valid extensions.
    for ext in VALID_CONFIG_FILE_EXTS {
        let path = path.with_extension(ext);
        if !path.is_file() {
            continue;
        }

        info!(path = %path.display(), "Found configuration file in load path.");
        return Ok(Some(path));
    }

    Ok(None)
}

/// Load a partial configuration from a file at `path`, if it exists.
///
/// This loads either the file directly, or tries to load a file with the same
/// name, but the extension replaced with one of the valid
/// `VALID_CONFIG_FILE_EXTS`.
pub fn load_partial_at_path<P: Into<PathBuf>>(path: P) -> Result<Option<PartialConfig>, Error> {
    open_config_file_at_path(path)?
        .map(|file| file.load::<PartialConfig>())
        .transpose()
        .map_err(Into::into)
}

/// Load a partial configuration from a file at `path`, recursing upwards until
/// either `/` or `root` is reached.
pub fn load_partial_at_path_recursive<P: Into<PathBuf>>(
    path: P,
    root: Option<&Path>,
) -> Result<Option<PartialConfig>, Error> {
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
        (Some(partial), Some(fallback)) => Ok(Some(partial.with_fallback(fallback))),

        // otherwise, return either one, or none.
        (partial, fallback) => Ok(partial.or(fallback)),
    }
}

/// Open a configuration file at `path`, if it exists.
///
/// If the file does not exist, the same file name is used but with one of the
/// valid `VALID_CONFIG_FILE_EXTS` extensions.
pub fn open_config_file_at_path<P: Into<PathBuf>>(path: P) -> Result<Option<File>, Error> {
    let mut path: PathBuf = path.into();

    trace!(path = %path.display(), "Trying to open configuration file.");
    if path.is_file() {
        info!(path = %path.display(), "Found configuration file.");
        return File::new(path).map(Some).map_err(Into::into);
    }

    for ext in VALID_CONFIG_FILE_EXTS {
        path.set_extension(ext);
        if !path.is_file() {
            continue;
        }

        info!(path = %path.display(), "Found configuration file.");
        return File::new(path).map(Some).map_err(Into::into);
    }

    Ok(None)
}

/// Expand tilde in path to home directory
///
/// If no tilde is found, returns `Some` with the original path. If a tilde is
/// found, but no home directory is set, returns `None`.
pub fn expand_tilde<T: AsRef<str>>(path: impl AsRef<str>, home: Option<T>) -> Option<PathBuf> {
    if path.as_ref().starts_with('~') {
        return home.map(|home| PathBuf::from(path.as_ref().replacen('~', home.as_ref(), 1)));
    }

    Some(PathBuf::from(path.as_ref()))
}

pub(crate) fn try_parse_vec<'a, T, E>(
    s: &'a str,
    parser: impl Fn(&'a str) -> std::result::Result<T, E>,
) -> std::result::Result<Vec<T>, Error>
where
    E: Into<Error>,
{
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parser(s).map_err(Into::into))
        .collect::<std::result::Result<Vec<_>, _>>()
}

/// Build a final configuration from merged partial configurations.
pub fn build(config: PartialConfig) -> Result<Config, Error> {
    let config = config.with_fallback(PartialConfig::default_values());
    let config = Config::from_partial(config)?;
    debug!(?config, "Loaded configuration.");

    Ok(config)
}

/// Get a file handle to the global config file, if it exists.
#[must_use]
pub fn user_global_config_path(home: Option<&Path>) -> Option<PathBuf> {
    env::var(GLOBAL_CONFIG_ENV_VAR)
        .ok()
        .and_then(|path| expand_tilde(path, home.and_then(Path::to_str)))
        .map(|path| path.clean())
        .inspect(|path| debug!(path = %path.display(), "Custom global configuration file path configured."))
        .or_else(||
            ProjectDirs::from("", "", APPLICATION)
                .map(|p| p.config_dir().join("config.toml"))
        )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use serial_test::serial;
    use tempfile::tempdir;
    use test_log::test;

    use super::*;

    struct EnvVarGuard {
        name: String,
        original_value: Option<String>,
    }

    impl EnvVarGuard {
        fn set(name: &str, value: &str) -> Self {
            let name = name.to_string();
            let original_value = env::var(&name).ok();
            unsafe { env::set_var(&name, value) };
            Self {
                name,
                original_value,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(ref original) = self.original_value {
                unsafe { env::set_var(&self.name, original) };
            } else {
                unsafe { env::remove_var(&self.name) };
            }
        }
    }

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
            partials: Vec<PartialConfig>,
            want: (&'static str, Option<Value>),
        }

        let cases = vec![
            ("disabled inheritance", TestCase {
                partials: vec![
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("FOO".to_owned());
                        partial
                    },
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("BAR".to_owned());
                        partial.inherit = Some(false);
                        partial
                    },
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("BAZ".to_owned());
                        partial
                    },
                ],
                want: (
                    "/assistant/provider/openrouter/api_key_env",
                    Some("BAR".into()),
                ),
            }),
            ("inheritance", TestCase {
                partials: vec![
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("FOO".to_owned());
                        partial
                    },
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("BAR".to_owned());
                        partial.inherit = Some(true);
                        partial
                    },
                    {
                        let mut partial = PartialConfig::empty();
                        partial.assistant.provider.openrouter.api_key_env = Some("BAZ".to_owned());
                        partial
                    },
                ],
                want: (
                    "/assistant/provider/openrouter/api_key_env",
                    Some("BAZ".into()),
                ),
            }),
        ];

        for (name, case) in cases {
            let partial = load_partials_with_inheritance(case.partials);
            let json = serde_json::to_value(&partial).unwrap();
            let val = json.pointer(case.want.0);

            assert_eq!(val, case.want.1.as_ref(), "failed case: {name}");
        }
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_envs() {
        let _env = EnvVarGuard::set("JP_ASSISTANT_PROVIDER_OPENROUTER_API_KEY_ENV", "ENV1");

        let partial = load_envs(PartialConfig::empty()).unwrap();
        assert_eq!(
            partial.assistant.provider.openrouter.api_key_env,
            Some("ENV1".to_owned())
        );
    }

    #[test]
    fn test_load_partial() {
        let partial = load_partial(PartialConfig::empty(), PartialConfig::default_values());
        assert_eq!(
            partial.assistant.provider.openrouter.api_key_env,
            Some("OPENROUTER_API_KEY".to_owned())
        );
    }

    #[test]
    fn test_build() {
        let config = build(PartialConfig::default_values()).unwrap();
        assert_eq!(
            config.assistant.provider.openrouter.api_key_env,
            "OPENROUTER_API_KEY".to_owned()
        );
    }

    #[test]
    fn test_expand_tilde() {
        struct TestCase {
            path: &'static str,
            home: Option<&'static str>,
            expected: Option<&'static str>,
        }

        let cases = vec![
            ("no tilde with home", TestCase {
                path: "no/tilde/here",
                home: Some("/tmp"),
                expected: Some("no/tilde/here"),
            }),
            ("no tilde missing home", TestCase {
                path: "no/tilde/here",
                home: None,
                expected: Some("no/tilde/here"),
            }),
            ("tilde path with home", TestCase {
                path: "~/subdir",
                home: Some("/tmp"),
                expected: Some("/tmp/subdir"),
            }),
            ("only tilde with home", TestCase {
                path: "~",
                home: Some("/tmp"),
                expected: Some("/tmp"),
            }),
            ("tilde missing home", TestCase {
                path: "~",
                home: None,
                expected: None,
            }),
        ];

        for (name, case) in cases {
            assert_eq!(
                expand_tilde(case.path, case.home),
                case.expected.map(PathBuf::from),
                "Failed test case: {name}"
            );
        }
    }

    #[test]
    fn test_find_file_in_path() {
        struct TestCase {
            segment: &'static str,
            load_path: &'static str,
            files: Vec<&'static str>,
            want: Result<Option<&'static str>, &'static str>,
        }

        let cases = vec![
            ("exact match", TestCase {
                segment: "config.toml",
                load_path: "foo",
                files: vec!["foo/config.toml"],
                want: Ok(Some("foo/config.toml")),
            }),
            ("exact match for any file type", TestCase {
                segment: "config.xxx",
                load_path: "foo",
                files: vec!["foo/config.xxx"],
                want: Ok(Some("foo/config.xxx")),
            }),
            ("match different supported file type", TestCase {
                segment: "config.xxx",
                load_path: "foo",
                files: vec!["foo/config.toml"],
                want: Ok(Some("foo/config.toml")),
            }),
            ("nested match", TestCase {
                segment: "bar/baz/config.toml",
                load_path: "foo",
                files: vec!["foo/bar/baz/config.toml"],
                want: Ok(Some("foo/bar/baz/config.toml")),
            }),
            ("does not recurse", TestCase {
                segment: "config.toml",
                load_path: "foo",
                files: vec!["foo/bar/baz/config.toml"],
                want: Ok(None),
            }),
            ("does not accept absolute segments", TestCase {
                segment: "/config.toml",
                load_path: "foo",
                files: vec!["foo/config.toml"],
                want: Ok(None),
            }),
        ];

        for (name, case) in cases {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            for file in case.files {
                write_config(&root.join(file), "");
            }

            let got = find_file_in_path(case.segment, root.join(case.load_path));
            if let Err(got) = &case.want {
                assert!(got.starts_with(got), "failed case: {name}");
                continue;
            }

            assert_eq!(
                got.map_err(|e| e.to_string())
                    .map(|v| v.map(|p| p.strip_prefix(root).unwrap().to_path_buf())),
                case.want
                    .map(|v| v.map(PathBuf::from))
                    .map_err(str::to_owned),
                "failed case: {name}",
            );
        }
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
                data: "assistant.provider.openrouter.api_key_env = 'FOO'",
                arg: "config.toml",
                want: Ok(Some("FOO")),
            }),
            ("exact match json", TestCase {
                file: "config.json",
                data: r#"{"assistant":{"provider":{"openrouter":{"api_key_env":"FOO"}}}}"#,
                arg: "config.json",
                want: Ok(Some("FOO")),
            }),
            ("exact match yaml", TestCase {
                file: "config.yaml",
                data: "assistant:\n  provider:\n    openrouter:\n      api_key_env: FOO",
                arg: "config.yaml",
                want: Ok(Some("FOO")),
            }),
            ("toml mismatch", TestCase {
                file: "config.toml",
                data: "assistant.provider.openrouter.api_key_env = 'FOO'",
                arg: "config.json",
                want: Ok(Some("FOO")),
            }),
            ("json mismatch", TestCase {
                file: "config.json",
                data: r#"{"assistant":{"provider":{"openrouter":{"api_key_env":"FOO"}}}}"#,
                arg: "config.yaml",
                want: Ok(Some("FOO")),
            }),
            ("yaml mismatch", TestCase {
                file: "config.yaml",
                data: "assistant:\n  provider:\n    openrouter:\n      api_key_env: FOO",
                arg: "config.toml",
                want: Ok(Some("FOO")),
            }),
            ("no extension", TestCase {
                file: "config.toml",
                data: "assistant.provider.openrouter.api_key_env = 'FOO'",
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
                want: Err("unknown configuration file format/extension:"),
            }),
        ];

        for (name, case) in cases {
            let tmp = tempdir().unwrap();
            let root = tmp.path();
            write_config(&root.join(case.file), case.data);

            let partial = load_partial_at_path(root.join(case.arg));
            if let Err(err) = &case.want {
                assert!(partial.is_err(), "failed case: {name}");
                assert!(
                    partial.unwrap_err().to_string().starts_with(err),
                    "failed case: {name}"
                );
                continue;
            }

            assert_eq!(
                partial
                    .map(|r| r.and_then(|p| p.assistant.provider.openrouter.api_key_env))
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
                        "assistant.provider.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"assistant":{"provider":{"openrouter":{"api_key_env":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/config.toml",
                root: None,
                want: Ok(Some((
                    "/assistant/provider/openrouter/api_key_env",
                    Some("FOO".into()),
                ))),
            }),
            ("merge different paths", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "assistant.provider.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"assistant":{"provider":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/config.toml",
                root: None,
                want: Ok(Some((
                    "/assistant/provider/openrouter",
                    Some(serde_json::json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
                ))),
            }),
            ("find upstream", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "assistant.provider.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"assistant":{"provider":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/bar/baz/config.yaml",
                root: None,
                want: Ok(Some((
                    "/assistant/provider/openrouter",
                    Some(serde_json::json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
                ))),
            }),
            ("merge until root", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "assistant.provider.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"assistant":{"provider":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo/bar/config.yaml",
                root: Some("foo"),
                want: Ok(Some((
                    "/assistant/provider/openrouter",
                    Some(serde_json::json!({"api_key_env": "FOO"})),
                ))),
            }),
            ("load dir instead of file", TestCase {
                files: vec![
                    (
                        "foo/config.toml",
                        "assistant.provider.openrouter.api_key_env = 'FOO'",
                    ),
                    (
                        "config.json",
                        r#"{"assistant":{"provider":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                    ),
                ],
                path: "foo",
                root: None,
                want: Ok(None),
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
