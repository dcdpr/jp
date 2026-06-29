#[cfg(unix)]
use serial_test::serial;
use test_log::test;

use super::*;
use crate::assignment::KvAssignment;
#[cfg(unix)]
use crate::util::EnvVarGuard;

#[test]
fn test_editor_config_cmd() {
    let mut p = PartialEditorConfig::default();

    let kv = KvAssignment::try_from_cli("cmd", "vim").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cmd, Some("vim".into()));

    let kv = KvAssignment::try_from_cli("cmd", "subl -w").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cmd, Some("subl -w".into()));
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

/// `cmd` runs through the shell and forwards the appended path via `"$@"`, so
/// the file being edited reaches the editor even with a bare-binary `cmd`.
#[cfg(unix)]
#[test]
fn command_cmd_forwards_appended_path() {
    let cfg = EditorConfig {
        cmd: Some("printf '<%s>'".into()),
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

/// Multiple edited paths (e.g.
/// `jp conversation edit`) are all forwarded.
#[cfg(unix)]
#[test]
fn command_cmd_forwards_multiple_paths() {
    let cfg = EditorConfig {
        cmd: Some("printf '<%s>'".into()),
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

/// A `cmd` that already references `"$@"` controls placement itself; the path
/// is not appended a second time.
#[cfg(unix)]
#[test]
fn command_cmd_with_explicit_args_is_not_double_appended() {
    let cfg = EditorConfig {
        cmd: Some(r#"printf '<%s>' "$@""#.into()),
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

/// A blank `cmd` falls through to `envs` rather than spawning an empty shell.
#[test]
fn command_blank_cmd_falls_through_to_envs() {
    let cfg = EditorConfig {
        cmd: Some("   ".into()),
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
