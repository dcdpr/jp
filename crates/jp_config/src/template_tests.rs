use super::*;
use crate::assignment::KvAssignment;

#[test]
fn test_template_config_values() {
    let mut p = PartialTemplateConfig::default();

    let kv = KvAssignment::try_from_cli("values.name", "Homer").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.values.as_ref().unwrap().get("name"),
        Some(&Value::String("Homer".to_string()))
    );
}
