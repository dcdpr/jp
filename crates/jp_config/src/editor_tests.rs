use test_log::test;

use super::*;
use crate::assignment::KvAssignment;

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
