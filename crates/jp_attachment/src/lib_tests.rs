use std::ffi::OsString;

use url::Url;

use super::*;

fn lookup(name: &str) -> Option<OsString> {
    match name {
        "PATH" => Some("/usr/bin".into()),
        "HOME" => Some("/home/jp".into()),
        "TERM" => Some("xterm".into()),
        "SECRET" => Some("hunter2".into()),
        _ => None,
    }
}

fn var(name: &str, value: &str) -> (OsString, OsString) {
    (name.into(), value.into())
}

/// `SECRET` is set, but nothing names it, so it stays out.
#[test]
fn child_env_holds_the_base_variables_and_the_named_ones() {
    let env = child_env_with(&["TERM".to_owned()], lookup);

    assert_eq!(env, vec![
        var("PATH", "/usr/bin"),
        var("HOME", "/home/jp"),
        var("TERM", "xterm"),
    ]);
}

#[test]
fn child_env_omits_a_named_variable_that_is_unset() {
    let env = child_env_with(&["UNSET".to_owned()], lookup);

    assert_eq!(env, vec![var("PATH", "/usr/bin"), var("HOME", "/home/jp")]);
}

#[test]
fn env_names_reads_every_env_parameter_in_order() {
    let uri = Url::parse("cmd://printenv?env=TERM&arg=TERM&env=JP_SESSION").unwrap();

    assert_eq!(env_names(&uri).unwrap(), ["TERM", "JP_SESSION"]);
}

/// A config table lists its names under one key, `params.envs = ["TERM",
/// "SHELL"]`, which becomes `envs=TERM&envs=SHELL`.
#[test]
fn env_names_reads_the_plural_spelling() {
    let uri = Url::parse("cmd://printenv?envs=TERM&env=JP_SESSION&envs=SHELL").unwrap();

    assert_eq!(env_names(&uri).unwrap(), ["TERM", "JP_SESSION", "SHELL"]);
}

/// The bracketed spellings of a repeated query parameter, as `arg[]` and
/// `args[]` are read for arguments.
#[test]
fn env_names_reads_the_bracketed_spellings() {
    let uri = Url::parse("cmd://printenv?env[]=TERM&envs[]=SHELL&env%5B%5D=JP_SESSION").unwrap();

    assert_eq!(env_names(&uri).unwrap(), ["TERM", "SHELL", "JP_SESSION"]);
}

#[test]
fn env_names_is_empty_without_the_parameter() {
    let uri = Url::parse("cmd://ls?arg=-l").unwrap();

    assert_eq!(env_names(&uri).unwrap(), Vec::<String>::new());
}

/// `env` passes a variable on with the value it already has; `NAME=value` reads
/// as an attempt to set one.
#[test]
fn env_names_rejects_an_assignment() {
    let uri = Url::parse("cmd://printenv?env=TERM=xterm").unwrap();

    assert_eq!(
        env_names(&uri).unwrap_err().to_string(),
        "invalid `env` value `TERM=xterm`: expected the name of an environment variable, such as \
         `TERM`"
    );
}

#[test]
fn env_names_reports_the_spelling_that_was_given() {
    let uri = Url::parse("cmd://printenv?envs=TERM=xterm").unwrap();

    assert_eq!(
        env_names(&uri).unwrap_err().to_string(),
        "invalid `envs` value `TERM=xterm`: expected the name of an environment variable, such as \
         `TERM`"
    );
}

#[test]
fn env_names_rejects_an_empty_name() {
    let uri = Url::parse("cmd://printenv?env=").unwrap();

    assert_eq!(
        env_names(&uri).unwrap_err().to_string(),
        "invalid `env` value ``: expected the name of an environment variable, such as `TERM`"
    );
}
