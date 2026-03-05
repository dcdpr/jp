use serial_test::serial;
use test_log::test;

use super::*;
use crate::{assignment::KvAssignment, util::EnvVarGuard};

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

#[test(serial(env_vars))]
fn test_editor_config_path() {
    let mut p = EditorConfig {
        cmd: Some("vim".into()),
        envs: vec![],
    };

    assert_eq!(p.path(), Some(Utf8PathBuf::from("vim")));

    p.cmd = Some("subl -w".into());
    assert_eq!(p.path(), Some(Utf8PathBuf::from("subl -w")));

    p.cmd = Some("/usr/bin/vim".into());
    assert_eq!(p.path(), Some(Utf8PathBuf::from("/usr/bin/vim")));

    p.cmd = None;
    p.envs = vec![];
    assert_eq!(p.path(), None);

    let _env = EnvVarGuard::set("JP_EDITOR1", "vi");
    p.envs = vec!["JP_EDITOR1".into()];
    assert!(p.path().unwrap().to_string().ends_with("/bin/vi"));

    let _env = EnvVarGuard::set("JP_EDITOR2", "doesnotexist");
    p.envs = vec!["JP_EDITOR2".into()];
    assert_eq!(p.path(), None);
}
