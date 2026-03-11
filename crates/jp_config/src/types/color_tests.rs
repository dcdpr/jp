use serde_json::{from_str, to_string};

use super::*;

#[test]
fn parse_ansi256_from_str() {
    assert_eq!("236".parse::<Color>().unwrap(), Color::Ansi256(236));
    assert_eq!("0".parse::<Color>().unwrap(), Color::Ansi256(0));
    assert_eq!("255".parse::<Color>().unwrap(), Color::Ansi256(255));
}

#[test]
fn parse_hex_rgb_from_str() {
    assert_eq!(
        "#504945".parse::<Color>().unwrap(),
        Color::Rgb {
            r: 80,
            g: 73,
            b: 69
        }
    );
    assert_eq!(
        "#FFFFFF".parse::<Color>().unwrap(),
        Color::Rgb {
            r: 255,
            g: 255,
            b: 255
        }
    );
    assert_eq!(
        "#000000".parse::<Color>().unwrap(),
        Color::Rgb { r: 0, g: 0, b: 0 }
    );
}

#[test]
fn parse_invalid() {
    assert!("256".parse::<Color>().is_err());
    assert!("-1".parse::<Color>().is_err());
    assert!("#50494".parse::<Color>().is_err());
    assert!("#GGGGGG".parse::<Color>().is_err());
    assert!("hello".parse::<Color>().is_err());
}

#[test]
fn to_ansi_bg_param_ansi256() {
    assert_eq!(Color::Ansi256(236).to_ansi_bg_param(), "48;5;236");
}

#[test]
fn to_ansi_bg_param_rgb() {
    let c = Color::Rgb {
        r: 80,
        g: 73,
        b: 69,
    };
    assert_eq!(c.to_ansi_bg_param(), "48;2;80;73;69");
}

#[test]
fn display_ansi256() {
    assert_eq!(Color::Ansi256(236).to_string(), "236");
}

#[test]
fn display_rgb() {
    let c = Color::Rgb {
        r: 80,
        g: 73,
        b: 69,
    };
    assert_eq!(c.to_string(), "#504945");
}

#[test]
fn serde_roundtrip_ansi256() {
    let c = Color::Ansi256(236);
    let json = to_string(&c).unwrap();
    assert_eq!(json, "236");
    assert_eq!(from_str::<Color>(&json).unwrap(), c);
}

#[test]
fn serde_roundtrip_rgb() {
    let c = Color::Rgb {
        r: 80,
        g: 73,
        b: 69,
    };
    let json = to_string(&c).unwrap();
    assert_eq!(json, "\"#504945\"");
    assert_eq!(from_str::<Color>(&json).unwrap(), c);
}

#[test]
fn deserialize_string_number() {
    // A number as a string should parse as Ansi256 via FromStr.
    assert_eq!(from_str::<Color>("\"236\"").unwrap(), Color::Ansi256(236));
}
