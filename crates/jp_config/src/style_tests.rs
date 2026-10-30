use serde_json::from_str;

use super::*;
use crate::{PartialAppConfig, assignment::KvAssignment, types::color::Color};

#[test]
fn test_link_style_deserialization() {
    assert_eq!(from_str::<LinkStyle>("false").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("true").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"off\"").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("\"full\"").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"osc8\"").unwrap(), LinkStyle::Osc8);
}

#[test]
fn test_markdown_elements_math_assign_and_toml() {
    let mut partial = PartialAppConfig::default();

    let kv = KvAssignment::try_from_cli("style.markdown.elements.math.fg", "yellow").unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(
        partial.style.markdown.elements.math.fg,
        Some(Color::Ansi256(3))
    );

    let kv = KvAssignment::try_from_cli("style.markdown.elements.math.bg", "236").unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(
        partial.style.markdown.elements.math.bg,
        Some(Color::Ansi256(236))
    );

    let parsed: PartialAppConfig = toml::from_str(
        r##"
        [style.markdown.elements.math]
        fg = "yellow"
        bg = "#3c3836"
        "##,
    )
    .unwrap();
    assert_eq!(
        parsed.style.markdown.elements.math.fg,
        Some(Color::Ansi256(3))
    );
    assert_eq!(
        parsed.style.markdown.elements.math.bg,
        Some(Color::Rgb {
            r: 0x3c,
            g: 0x38,
            b: 0x36,
        })
    );
}
