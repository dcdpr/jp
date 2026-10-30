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
    assert_eq!("#504945".parse::<Color>().unwrap(), Color::Rgb {
        r: 80,
        g: 73,
        b: 69
    });
    assert_eq!("#FFFFFF".parse::<Color>().unwrap(), Color::Rgb {
        r: 255,
        g: 255,
        b: 255
    });
    assert_eq!("#000000".parse::<Color>().unwrap(), Color::Rgb {
        r: 0,
        g: 0,
        b: 0
    });
}

#[test]
fn parse_named_ansi_from_str() {
    assert_eq!("black".parse::<Color>().unwrap(), Color::Ansi256(0));
    assert_eq!("red".parse::<Color>().unwrap(), Color::Ansi256(1));
    assert_eq!("green".parse::<Color>().unwrap(), Color::Ansi256(2));
    assert_eq!("yellow".parse::<Color>().unwrap(), Color::Ansi256(3));
    assert_eq!("YELLOW".parse::<Color>().unwrap(), Color::Ansi256(3));
    assert_eq!("blue".parse::<Color>().unwrap(), Color::Ansi256(4));
    assert_eq!("magenta".parse::<Color>().unwrap(), Color::Ansi256(5));
    assert_eq!("cyan".parse::<Color>().unwrap(), Color::Ansi256(6));
    assert_eq!("white".parse::<Color>().unwrap(), Color::Ansi256(7));
    assert_eq!("bright_black".parse::<Color>().unwrap(), Color::Ansi256(8));
    assert_eq!("bright-black".parse::<Color>().unwrap(), Color::Ansi256(8));
    assert_eq!("gray".parse::<Color>().unwrap(), Color::Ansi256(8));
    assert_eq!("grey".parse::<Color>().unwrap(), Color::Ansi256(8));
    assert_eq!("bright_red".parse::<Color>().unwrap(), Color::Ansi256(9));
    assert_eq!("bright_green".parse::<Color>().unwrap(), Color::Ansi256(10));
    assert_eq!(
        "bright_yellow".parse::<Color>().unwrap(),
        Color::Ansi256(11)
    );
    assert_eq!(
        "bright-yellow".parse::<Color>().unwrap(),
        Color::Ansi256(11)
    );
    assert_eq!("bright_blue".parse::<Color>().unwrap(), Color::Ansi256(12));
    assert_eq!(
        "bright_magenta".parse::<Color>().unwrap(),
        Color::Ansi256(13)
    );
    assert_eq!("bright_cyan".parse::<Color>().unwrap(), Color::Ansi256(14));
    assert_eq!("bright_white".parse::<Color>().unwrap(), Color::Ansi256(15));
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
    assert_eq!(from_str::<Color>("\"236\"").unwrap(), Color::Ansi256(236));
}
