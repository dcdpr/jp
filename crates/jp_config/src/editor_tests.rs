#[cfg(unix)]
use serial_test::serial;
use test_log::test;

use super::*;
use crate::assignment::KvAssignment;
#[cfg(unix)]
use crate::types::command::CommandConfig;
#[cfg(unix)]
use crate::util::EnvVarGuard;

/// A string `cmd` is stored verbatim on the partial; splitting into
/// program/args happens later in `command()`.
fn string_cmd(s: &str) -> CommandConfigOrString {
    CommandConfigOrString::String(s.to_owned())
}

#[cfg(unix)]
fn table_cmd(program: &str, args: &[&str], shell: bool) -> CommandConfigOrString {
    CommandConfigOrString::Config(CommandConfig {
        program: program.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        shell,
    })
}

#[test]
fn test_editor_config_cmd() {
    let mut p = PartialEditorConfig::default();

    let kv = KvAssignment::try_from_cli("cmd", "vim").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.cmd,
        Some(PartialCommandConfigOrString::String("vim".to_owned()))
    );

    let kv = KvAssignment::try_from_cli("cmd", "subl -w").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.cmd,
        Some(PartialCommandConfigOrString::String("subl -w".to_owned()))
    );
}

#[test]
fn test_editor_config_envs() {
    let mut p = PartialEditorConfig::default();

    let kv = KvAssignment::try_from_cli("envs", "EDITOR,VISUAL").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.envs, Some(vec!["EDITOR".into(), "VISUAL".into()]));

    let kv = KvAssignment::try_from_cli("envs:", r#"["EDITOR","VISUAL"]"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.envs, Some(vec!["EDITOR".into(), "VISUAL".into()]));

    let kv = KvAssignment::try_from_cli("envs.0", "EDIT").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.envs, Some(vec!["EDIT".into(), "VISUAL".into()]));

    let kv = KvAssignment::try_from_cli("envs+:", r#"["OTHER"]"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.envs,
        Some(vec!["EDIT".into(), "VISUAL".into(), "OTHER".into()])
    );

    let kv = KvAssignment::try_from_cli("envs+", "LAST").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.envs,
        Some(vec![
            "EDIT".into(),
            "VISUAL".into(),
            "OTHER".into(),
            "LAST".into()
        ])
    );
}

// --- `command()` argument forwarding -------------------------------------

/// A string `cmd` (the default `shell = false`) spawns the program directly and
/// the edited path is appended as a trailing argument.
#[cfg(unix)]
#[test]
fn command_cmd_string_forwards_appended_path() {
    let cfg = EditorConfig {
        cmd: Some(string_cmd("printf '<%s>'")),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<FILE>");
}

/// Multiple edited paths (e.g. `jp conversation edit`) are all appended.
#[cfg(unix)]
#[test]
fn command_cmd_string_forwards_multiple_paths() {
    let cfg = EditorConfig {
        cmd: Some(string_cmd("printf '<%s>'")),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("A");
            cmd.arg("B");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<A><B>");
}

/// The table form with explicit `args` forwards the appended path the same way.
#[cfg(unix)]
#[test]
fn command_cmd_table_forwards_appended_path() {
    let cfg = EditorConfig {
        cmd: Some(table_cmd("printf", &["<%s>"], false)),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<FILE>");
}

/// A missing program with `shell = false` is a spawn error, not a silent
/// non-zero exit — this is the typo'd-`editor.cmd` case.
#[cfg(unix)]
#[test]
fn command_cmd_missing_binary_is_spawn_error() {
    let cfg = EditorConfig {
        cmd: Some(string_cmd("jp-no-such-editor-binary-xyz")),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let result = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .stdout_null()
        .stderr_null()
        .run();

    assert!(result.is_err());
}

/// With `shell = true` the command runs through `/bin/sh` and the appended path
/// is forwarded via `"$@"`.
#[cfg(unix)]
#[test]
fn command_cmd_shell_forwards_via_dollar_at() {
    let cfg = EditorConfig {
        cmd: Some(table_cmd("printf '<%s>'", &[], true)),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<FILE>");
}

/// A `shell = true` command that already references `"$@"` controls placement
/// itself; the path is not appended a second time.
#[cfg(unix)]
#[test]
fn command_cmd_shell_explicit_dollar_at_is_not_double_appended() {
    let cfg = EditorConfig {
        cmd: Some(table_cmd(r#"printf '<%s>' "$@""#, &[], true)),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<FILE>");
}

/// A `shell = true` command shell-quotes its discrete `args`, so a multi-word
/// argument keeps its boundary instead of being word-split by the shell.
#[cfg(unix)]
#[test]
fn command_cmd_shell_quotes_multiword_args() {
    let cfg = EditorConfig {
        cmd: Some(table_cmd("printf", &["<%s>", "a b"], true)),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    // Quoted: `printf '<%s>' 'a b' FILE` -> the space-containing arg stays one
    // token. A naive join would yield `<a><b><FILE>`.
    assert_eq!(out, "<a b><FILE>");
}

/// A blank `cmd` falls through to `envs` rather than spawning an empty program.
#[test]
fn command_blank_cmd_falls_through_to_envs() {
    let cfg = EditorConfig {
        cmd: Some(string_cmd("   ")),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    };

    assert!(cfg.command().is_none());
}

/// The env-var branch preserves configured arguments *and* appends the edited
/// path, so a multi-arg editor like `emacsclient --nw` opens the file.
#[cfg(unix)]
#[test(serial(env_vars))]
fn command_env_preserves_args_and_forwards_path() {
    let _env = EnvVarGuard::set("JP_EDITOR_TEST", r#"sh -c 'printf "<%s>" "$@"' inner"#);
    let cfg = EditorConfig {
        cmd: None,
        envs: vec!["JP_EDITOR_TEST".into()],
        inline: InlineEditorConfig::default(),
    };

    let out = cfg
        .command()
        .unwrap()
        .before_spawn(|cmd| {
            cmd.arg("FILE");
            Ok(())
        })
        .read()
        .unwrap();

    assert_eq!(out, "<FILE>");
}

/// Env-var values with unbalanced quoting are skipped (no command resolves).
#[cfg(unix)]
#[test(serial(env_vars))]
fn command_env_skips_unbalanced_quoting() {
    let _env = EnvVarGuard::set("JP_EDITOR_TEST", "vi 'unterminated");
    let cfg = EditorConfig {
        cmd: None,
        envs: vec!["JP_EDITOR_TEST".into()],
        inline: InlineEditorConfig::default(),
    };

    assert!(cfg.command().is_none());
}

/// An env-var editor whose binary is not on `PATH` is skipped.
#[cfg(unix)]
#[test(serial(env_vars))]
fn command_env_skips_missing_binary() {
    let _env = EnvVarGuard::set("JP_EDITOR_TEST", "jp-no-such-editor-binary");
    let cfg = EditorConfig {
        cmd: None,
        envs: vec!["JP_EDITOR_TEST".into()],
        inline: InlineEditorConfig::default(),
    };

    assert!(cfg.command().is_none());
}
