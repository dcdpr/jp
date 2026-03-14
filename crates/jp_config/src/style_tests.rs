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
