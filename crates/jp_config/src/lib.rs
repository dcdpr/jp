mod config;
mod conversation;
pub mod editor;
pub mod error;
pub mod llm;
mod parse;
pub mod style;
mod template;

pub use config::Config;
pub use error::Error;
pub use parse::{
    build, expand_tilde, file_to_key_value_pairs, load, load_envs, load_partial, parse_vec,
    try_parse_vec, PartialConfig,
};

fn set_error(path: &str, key: impl Into<String>) -> error::Result<()> {
    let s: String = key.into();

    Err(Error::UnknownConfigKey {
        key: s.clone(),
        available_keys: {
            let mut keys = Config::fields();
            let mut path = Some(path);
            while let Some(prefix) = path {
                path = prefix.rsplit_once('.').map(|(prefix, _)| prefix);

                let matches = Config::fields()
                    .into_iter()
                    .filter(|f| f.starts_with(prefix))
                    .collect::<Vec<_>>();

                if !matches.is_empty() {
                    keys = matches;
                    break;
                }
            }

            keys
        },
    })
}
