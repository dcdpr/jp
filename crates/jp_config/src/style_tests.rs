use serde_json::from_str;

use super::*;
use crate::{AppConfig, PartialAppConfig};

#[test]
fn test_link_style_deserialization() {
    assert_eq!(from_str::<LinkStyle>("false").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("true").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"off\"").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("\"full\"").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"osc8\"").unwrap(), LinkStyle::Osc8);
}

#[test]
fn sanitize_defaults_to_strip() {
    assert_eq!(AppConfig::new_test().style.sanitize, Sanitization::Strip);
}

#[test]
fn sanitize_is_read_from_the_style_table() {
    let partial: PartialAppConfig = toml::from_str("[style]\nsanitize = \"visualize\"\n").unwrap();

    assert_eq!(partial.style.sanitize, Some(Sanitization::Visualize));
}

#[test]
fn sanitize_accepts_strip_visualize_and_off() {
    assert_eq!(
        from_str::<Sanitization>(r#""strip""#).unwrap(),
        Sanitization::Strip
    );
    assert_eq!(
        from_str::<Sanitization>(r#""visualize""#).unwrap(),
        Sanitization::Visualize
    );
    assert_eq!(
        from_str::<Sanitization>(r#""off""#).unwrap(),
        Sanitization::Off
    );
}

#[test]
fn sanitize_rejects_anything_else() {
    // A boolean is refused rather than read as `off` or `strip`: the setting
    // names one of three modes, and a boolean names none of them.
    assert!(from_str::<Sanitization>("false").is_err());
    assert!(from_str::<Sanitization>(r#""none""#).is_err());
}

#[test]
fn sanitize_is_written_as_its_name() {
    // A conversation stores its config changes in this form.
    assert_eq!(
        serde_json::to_string(&Sanitization::Visualize).unwrap(),
        r#""visualize""#
    );
}

#[test]
fn sanitize_can_be_set_with_cfg() {
    // `JP_CFG_STYLE_SANITIZE` goes through the same assignment.
    let mut partial = PartialAppConfig::default();

    partial
        .assign(KvAssignment::try_from_cli("style.sanitize", "off").unwrap())
        .unwrap();

    assert_eq!(partial.style.sanitize, Some(Sanitization::Off));
}

#[test]
fn sanitize_refuses_an_unknown_value_from_cfg() {
    let mut partial = PartialAppConfig::default();

    let result = partial.assign(KvAssignment::try_from_cli("style.sanitize", "none").unwrap());

    assert!(result.is_err());
    assert_eq!(partial.style.sanitize, None);
}

#[test]
fn a_changed_sanitize_is_recorded_in_the_delta() {
    let prev = AppConfig::new_test();
    let mut next = prev.clone();
    next.style.sanitize = Sanitization::Visualize;

    let changed = prev.to_partial().delta(next.to_partial());
    let unchanged = prev.to_partial().delta(prev.to_partial());

    assert_eq!(changed.style.sanitize, Some(Sanitization::Visualize));
    assert_eq!(unchanged.style.sanitize, None);
}

#[test]
fn an_unset_sanitize_is_filled_from_the_defaults() {
    let defaults = PartialStyleConfig {
        sanitize: Some(Sanitization::Off),
        ..PartialStyleConfig::default()
    };

    let unset = PartialStyleConfig::default().fill_from(defaults.clone());
    let set = PartialStyleConfig {
        sanitize: Some(Sanitization::Visualize),
        ..PartialStyleConfig::default()
    }
    .fill_from(defaults);

    assert_eq!(unset.sanitize, Some(Sanitization::Off));
    assert_eq!(set.sanitize, Some(Sanitization::Visualize));
}
