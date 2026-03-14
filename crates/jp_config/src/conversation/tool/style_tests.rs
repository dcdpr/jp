use serde_json::from_str;

use super::*;

#[test]
fn test_link_style_deserialization() {
    assert_eq!(from_str::<LinkStyle>("false").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("true").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"off\"").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("\"full\"").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"osc8\"").unwrap(), LinkStyle::Osc8);
}

#[test]
fn test_inline_results_deserialization() {
    assert_eq!(
        from_str::<InlineResults>("false").unwrap(),
        InlineResults::Off
    );
    assert_eq!(
        from_str::<InlineResults>("true").unwrap(),
        InlineResults::Full
    );
    assert_eq!(
        from_str::<InlineResults>("\"off\"").unwrap(),
        InlineResults::Off
    );
    assert_eq!(
        from_str::<InlineResults>("\"full\"").unwrap(),
        InlineResults::Full
    );
    assert_eq!(
        from_str::<InlineResults>("10").unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 10 })
    );
    assert_eq!(
        from_str::<InlineResults>("\"25\"").unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 25 })
    );
    assert_eq!(
        from_str::<InlineResults>(r#"{"truncate": {"lines": 5}}"#).unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 5 })
    );
}
