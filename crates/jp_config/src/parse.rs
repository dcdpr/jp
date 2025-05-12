use std::{
    convert::Infallible,
    env,
    path::{Path, PathBuf},
};

use confique::{Config as Confique, File, Partial as _};
use directories::ProjectDirs;
use path_clean::PathClean as _;
use tracing::{debug, info, trace};

use super::{error::Result, Config};

pub type PartialConfig = <Config as Confique>::Partial;

const APPLICATION: &str = "jp";
const GLOBAL_CONFIG_ENV_VAR: &str = "JP_GLOBAL_CONFIG_FILE";
const VALID_CONFIG_FILE_STEMS_GLOBAL: &[&str] = &["config"];
const VALID_CONFIG_FILE_STEMS_LOCAL: &[&str] = &["jp", ".jp"];
const VALID_CONFIG_FILE_EXTS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

/// Load configuration, respecting the hierarchical inheritance chain
///
/// If `search` is true, the function will walk up the directory tree to find
/// the configuration file, before returning the final configuration.
pub fn load(root: &Path, search: bool) -> Result<Config> {
    trace!(root = %root.display(), ?search, "Loading configuration.");

    build(load_envs(load_partial(root, search, None)?)?)
}

/// Load a partial configuration for a given root directory.
pub fn load_partial(
    root: &Path,
    search: bool,
    base: Option<PartialConfig>,
) -> Result<PartialConfig> {
    trace!(root = %root.display(), ?search, "Loading partial configuration.");

    let mut inherit = search;
    let mut partials = Vec::new();

    // Load config file present at `root`.
    if let Some(file) = open_config_file(root, VALID_CONFIG_FILE_STEMS_LOCAL)? {
        partials.push(file.load::<PartialConfig>()?);
    }

    // Start with local configs by walking up the directory tree.
    if inherit {
        if let Some(root) = root.parent() {
            inherit = search_config_file(root, &mut partials)?;
        }
    }

    // Add global config if inheritance is allowed, and file is found.
    if inherit {
        if let Some(file) = open_global_config_file(env::var("HOME").ok().as_deref())? {
            partials.push(file.load::<PartialConfig>()?);
        }
    }

    // Merge all partials in reverse order (most general to most specific).
    let mut merged = base.unwrap_or(PartialConfig::default_values());
    for partial in partials.into_iter().rev() {
        merged = partial.with_fallback(merged);
    }

    Ok(merged)
}

/// Load environment variables into a partial configuration.
pub fn load_envs(base: PartialConfig) -> Result<PartialConfig> {
    trace!("Loading environment variable configuration.");

    Ok(PartialConfig::from_env()?.with_fallback(base))
}

/// Build a final configuration from merged partial configurations.
pub fn build(config: PartialConfig) -> Result<Config> {
    let config = Config::from_partial(config)?;
    debug!(?config, "Loaded configuration.");

    Ok(config)
}

/// Search for a config file, starting at `root` and working up the directory
/// tree.
///
/// All config files found are loaded pushed into the `partials` vector, until a
/// config file is found that has `inherit = false`.
fn search_config_file(root: &Path, partials: &mut Vec<PartialConfig>) -> Result<bool> {
    let mut inherit = true;
    if let Some(file) = open_config_file(root, VALID_CONFIG_FILE_STEMS_LOCAL)? {
        let partial = file.load::<PartialConfig>()?;
        inherit = partial.inherit.unwrap_or(true);
        partials.push(partial);
    }

    let Some(parent) = root.parent() else {
        return Ok(inherit);
    };

    if inherit {
        inherit = search_config_file(parent, partials)?;
    }

    Ok(inherit)
}

/// Get a file handle to the config file at `path`, if it exists.
///
/// If `path` is a file, it is opened, or an error is returned.
///
/// If `path` is a directory, the first file with a valid
/// [`VALID_CONFIG_FILE_EXTS`] extension is opened, if any.
fn open_config_file(path: &Path, stems: &[&str]) -> Result<Option<File>> {
    trace!(path = %path.display(), "Searching for configuration file.");

    if path.is_file() {
        info!(path = %path.display(), "Found configuration file.");
        return File::new(path).map(Some).map_err(Into::into);
    }

    for stem in stems {
        for ext in VALID_CONFIG_FILE_EXTS {
            let path = path.join(format!("{stem}.{ext}"));
            if !path.is_file() {
                continue;
            }

            info!(path = %path.display(), "Found configuration file.");
            return File::new(path).map(Some).map_err(Into::into);
        }
    }

    Ok(None)
}

/// Get a file handle to the global config file, if it exists.
fn open_global_config_file(home: Option<&str>) -> Result<Option<File>> {
    env::var(GLOBAL_CONFIG_ENV_VAR)
        .ok()
        .and_then(|path| expand_tilde(path, home))
        .map(|path| path.clean())
        .inspect(|path| debug!(path = %path.display(), "Custom global configuration file path configured."))
        .or_else(|| {
            ProjectDirs::from("", "", APPLICATION)
                .map(|p| p.config_dir().to_path_buf())
        })
        .map(|path| open_config_file(&path, VALID_CONFIG_FILE_STEMS_GLOBAL))
        .transpose()
        .map(Option::flatten)
}

/// Expand tilde in path to home directory
///
/// If no tilde is found, returns `Some` with the original path. If a tilde is
/// found, but no home directory is set, returns `None`.
fn expand_tilde(path: impl AsRef<str>, home: Option<&str>) -> Option<PathBuf> {
    if path.as_ref().starts_with('~') {
        return home.map(|home| PathBuf::from(path.as_ref().replacen('~', home, 1)));
    }

    Some(PathBuf::from(path.as_ref()))
}

#[expect(clippy::missing_panics_doc)]
pub fn parse_vec<'a, T>(s: &'a str, parser: impl Fn(&'a str) -> T) -> Vec<T> {
    try_parse_vec(s, |s| Ok::<_, Infallible>(parser(s))).expect("infallible parser")
}

pub fn try_parse_vec<'a, T, E>(
    s: &'a str,
    parser: impl Fn(&'a str) -> std::result::Result<T, E>,
) -> std::result::Result<Vec<T>, Box<dyn std::error::Error + Send + Sync>>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parser(s).map_err(Into::into))
        .collect::<std::result::Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serial_test::serial;
    use tempfile::tempdir;

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
    fn test_expand_tilde_no_tilde() {
        let path = "no/tilde/here";
        assert_eq!(expand_tilde(path, Some("/tmp")), Some(PathBuf::from(path)));
    }

    #[test]
    fn test_expand_tilde_with_tilde() {
        let home = "/tmp";
        let path = "~/subdir";
        let expected = PathBuf::from(home).join("subdir");
        assert_eq!(expand_tilde(path, Some(home)), Some(expected));
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        let home = "/tmp";
        let path = "~";
        let expected = PathBuf::from(home);
        assert_eq!(expand_tilde(path, Some(home)), Some(expected));
    }

    #[test]
    fn test_expand_tilde_missing_home() {
        let path = "~/no_home";
        assert_eq!(expand_tilde(path, None), None);
    }

    #[test]
    fn test_open_config_file_direct_path() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("my_config.toml");
        write_config(&cfg, "llm.provider.openrouter.api_key_env = 'FOO'");

        let file = open_config_file(&cfg, &[]).unwrap().unwrap();
        let partial = file.load::<PartialConfig>().unwrap();
        assert_eq!(partial.llm.provider.openrouter.api_key_env.unwrap(), "FOO");
    }

    #[test]
    fn test_open_config_file_in_dir_default_names() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        for stem in VALID_CONFIG_FILE_STEMS_LOCAL {
            for ext in VALID_CONFIG_FILE_EXTS {
                let data = match *ext {
                    "toml" => "llm.provider.openrouter.api_key_env = 'BAR'",
                    "yaml" | "yml" => "llm:\n  provider:\n    openrouter:\n      api_key_env: BAR",
                    "json5" => "{ llm: { provider: { openrouter: { api_key_env: 'BAR' } } } }",
                    "json" => {
                        r#"{ "llm": { "provider": { "openrouter": { "api_key_env": "BAR" } } } }"#
                    }
                    _ => panic!("Untested extension: {ext}"),
                };

                let path = root.join(format!("{stem}.{ext}"));
                write_config(&path, data);

                let file = open_config_file(root, VALID_CONFIG_FILE_STEMS_LOCAL)
                    .unwrap()
                    .unwrap();
                let partial = file.load::<PartialConfig>().unwrap();
                assert_eq!(partial.llm.provider.openrouter.api_key_env.unwrap(), "BAR");
                fs::remove_file(path).unwrap();
            }
        }
    }

    #[test]
    fn test_open_config_file_not_found() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let cfg = root.join("jp.toml");

        assert!(open_config_file(root, VALID_CONFIG_FILE_STEMS_LOCAL)
            .unwrap()
            .is_none()); // No file in dir
        assert!(open_config_file(&cfg, VALID_CONFIG_FILE_STEMS_LOCAL)
            .unwrap()
            .is_none()); // File doesn't exist
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_no_files_yields_defaults() {
        let tmp = tempdir().unwrap();
        let root = tmp.path(); // No config files created

        let config = load(root, true).unwrap(); // Search enabled

        assert!(config.inherit);
        assert_eq!(
            config.llm.provider.openrouter.api_key_env,
            "OPENROUTER_API_KEY"
        );
        assert_eq!(config.llm.provider.openrouter.app_name, "JP");
        assert_eq!(config.llm.provider.openrouter.app_referrer, None);
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_single_file_at_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_config(
            &root.join("jp.toml"),
            "llm.provider.openrouter.api_key_env = 'ROOT_KEY'",
        );

        let config = load(root, false).unwrap(); // Search disabled

        assert_eq!(config.llm.provider.openrouter.api_key_env, "ROOT_KEY");
        assert!(config.inherit); // Default inherit=true
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_hierarchy_and_merge() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("foo/bar/workspace");
        fs::create_dir_all(&root).unwrap();
        let bar = root.parent().unwrap();
        let foo = bar.parent().unwrap();

        write_config(
            &foo.join(".jp.toml"), // Lowest precedence
            "inherit = true\nllm.provider.openrouter.app_name = 'GRANDPARENT'",
        );
        write_config(
            &bar.join(".jp.toml"), // Middle precedence
            "llm.provider.openrouter.api_key_env = 'PARENT_KEY'\nllm.provider.openrouter.app_name \
             = 'PARENT'", // Overrides grandparent name
        );
        write_config(
            &root.join("jp.toml"),                              // Highest precedence
            "llm.provider.openrouter.api_key_env = 'ROOT_KEY'", // Overrides parent key_env
        );

        let config = load(&root, true).unwrap(); // Search enabled

        // FIXME: This `inherit` assertion is wrong, because it also defaults to
        // `true`, so this isn't really testing the grandparent inheritance.
        //
        // But, we don't have any other fields to test against, so we'll change
        // this in the future when the config struct expands.
        assert!(config.inherit); // From grandparent
        assert_eq!(config.llm.provider.openrouter.api_key_env, "ROOT_KEY"); // From root
        assert_eq!(config.llm.provider.openrouter.app_name, "PARENT"); // From parent
        assert_eq!(config.llm.provider.openrouter.app_referrer, None); // Default
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_inherit_false_stops_search() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("foo/bar/workspace");
        fs::create_dir_all(&root).unwrap();
        let bar = root.parent().unwrap();
        let foo = bar.parent().unwrap();

        write_config(
            &foo.join(".jp.toml"), // Should NOT be loaded
            "llm.provider.openrouter.api_key_env = 'GRANDPARENT_KEY'",
        );
        write_config(
            &bar.join(".jp.toml"), // Should be loaded, and stop search
            "inherit = false\nllm.provider.openrouter.app_name = 'PARENT'",
        );
        write_config(
            &root.join("jp.toml"), // Should be loaded (most specific)
            "llm.provider.openrouter.api_key_env = 'ROOT_KEY'",
        );

        let config = load(&root, true).unwrap(); // Search enabled

        assert!(!config.inherit); // From parent config file
        assert_eq!(config.llm.provider.openrouter.api_key_env, "ROOT_KEY"); // From root config

        // Grandparent key_env should not be present
        assert_eq!(config.llm.provider.openrouter.app_name, "PARENT"); // From parent config
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_disable_search() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project/workspace");
        fs::create_dir_all(&root).unwrap();
        let parent = root.parent().unwrap();

        write_config(
            &parent.join(".jp.toml"), // Should NOT be loaded
            "llm.provider.openrouter.api_key_env = 'PARENT_KEY'",
        );
        write_config(
            &root.join("jp.toml"), // Should be loaded
            "llm.provider.openrouter.app_name = 'ROOT_APP'",
        );

        let config = load(&root, false).unwrap(); // Search disabled

        assert_eq!(config.llm.provider.openrouter.app_name, "ROOT_APP"); // From root
        assert_eq!(
            config.llm.provider.openrouter.api_key_env,
            "OPENROUTER_API_KEY"
        ); // Default (parent not loaded)
        assert!(config.inherit); // Default
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_env_var_overrides_file() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_config(
            &root.join("config.toml"),
            "llm.provider.openrouter.api_key_env = 'FILE_KEY'\nllm.provider.openrouter.app_name = \
             'FILE_APP'",
        );

        let _guard1 = EnvVarGuard::set("JP_LLM_PROVIDER_OPENROUTER_API_KEY_ENV", "ENV_KEY");
        let _guard2 = EnvVarGuard::set("JP_LLM_PROVIDER_OPENROUTER_APP_NAME", "ENV_APP");

        // Load with search disabled to only consider root file + env
        let config = load(root, false).unwrap();

        assert_eq!(config.llm.provider.openrouter.api_key_env, "ENV_KEY"); // Overridden
        assert_eq!(config.llm.provider.openrouter.app_name, "ENV_APP"); // Overridden
        assert_eq!(config.llm.provider.openrouter.app_referrer, None); // Default (not set)
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_env_var_only() {
        let tmp = tempdir().unwrap();
        let root = tmp.path(); // No config files

        let _guard1 = EnvVarGuard::set("JP_LLM_PROVIDER_OPENROUTER_API_KEY_ENV", "ENV_KEY_ONLY");
        let _guard2 = EnvVarGuard::set(
            "JP_LLM_PROVIDER_OPENROUTER_APP_REFERRER",
            "http://example.com",
        );

        let config = load(root, true).unwrap(); // Search enabled, but finds nothing

        assert_eq!(config.llm.provider.openrouter.api_key_env, "ENV_KEY_ONLY"); // From env
        assert_eq!(config.llm.provider.openrouter.app_name, "JP"); // Default (not set)
        assert_eq!(
            config.llm.provider.openrouter.app_referrer,
            Some("http://example.com".to_string())
        ); // From env
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_env_var_overrides_even_with_inherit_false() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project/workspace");
        fs::create_dir_all(&root).unwrap();
        let parent = root.parent().unwrap();

        write_config(
            &parent.join(".jp.toml"), // Should NOT be loaded by file search
            "llm.provider.openrouter.api_key_env = 'PARENT_KEY'",
        );
        write_config(
            &root.join("jp.toml"), // Should be loaded, sets inherit=false
            "inherit = false\nllm.provider.openrouter.app_name = 'ROOT_APP'",
        );

        // Env var should still override the value from root file
        let _guard1 = EnvVarGuard::set("JP_LLM_PROVIDER_OPENROUTER_APP_NAME", "ENV_APP_OVERRIDE");
        // Env var should provide value even though parent file wasn't loaded
        let _guard2 = EnvVarGuard::set(
            "JP_LLM_PROVIDER_OPENROUTER_API_KEY_ENV",
            "ENV_KEY_INHERIT_FALSE",
        );

        let config = load(&root, true).unwrap(); // Search enabled

        assert!(!config.inherit); // From root file
        assert_eq!(config.llm.provider.openrouter.app_name, "ENV_APP_OVERRIDE"); // Env overrides root file
        assert_eq!(
            config.llm.provider.openrouter.api_key_env,
            "ENV_KEY_INHERIT_FALSE"
        ); // Env provides value despite inherit=false
    }

    #[test]
    #[serial(env_vars)]
    fn test_load_precedence_file_over_file_env_over_all() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project/workspace");
        fs::create_dir_all(&root).unwrap();
        let parent = root.parent().unwrap();

        write_config(
            &parent.join(".jp.toml"), // Loaded first (lowest file precedence)
            "llm.provider.openrouter.api_key_env = 'PARENT_KEY'\nllm.provider.openrouter.app_name \
             = 'PARENT_APP'",
        );
        write_config(
            &root.join("config.toml"), // Loaded second (highest file precedence)
            "llm.provider.openrouter.app_name = 'ROOT_APP'", // Overrides parent app_name
        );

        // Env var overrides both files
        let _guard = EnvVarGuard::set("JP_LLM_PROVIDER_OPENROUTER_APP_NAME", "ENV_APP_FINAL");

        let config = load(&root, true).unwrap(); // Search enabled

        assert!(config.inherit); // Default from parent
        assert_eq!(config.llm.provider.openrouter.api_key_env, "PARENT_KEY"); // From parent file (not overridden by root file or env)
        assert_eq!(config.llm.provider.openrouter.app_name, "ENV_APP_FINAL"); // Env overrides root file override of parent file
    }
}
