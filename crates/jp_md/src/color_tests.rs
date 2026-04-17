use syntect::highlighting::FontStyle;

use super::*;

// -- ColorMode::detect ----------------------------------------------------

#[test]
fn detect_truecolor_from_colorterm() {
    assert_eq!(
        ColorMode::detect(Some("truecolor"), Some("xterm-256color")),
        ColorMode::TrueColor,
    );
    assert_eq!(ColorMode::detect(Some("24bit"), None), ColorMode::TrueColor,);
}

#[test]
fn detect_256color_from_term() {
    assert_eq!(
        ColorMode::detect(None, Some("xterm-256color")),
        ColorMode::Ansi256,
    );
    assert_eq!(
        ColorMode::detect(None, Some("screen-256color")),
        ColorMode::Ansi256,
    );
}

#[test]
fn detect_truecolor_when_no_hints() {
    assert_eq!(ColorMode::detect(None, None), ColorMode::TrueColor);
    assert_eq!(ColorMode::detect(None, Some("xterm")), ColorMode::TrueColor);
}

#[test]
fn colorterm_takes_priority_over_term() {
    // iTerm2 sets both COLORTERM=truecolor and TERM=xterm-256color.
    assert_eq!(
        ColorMode::detect(Some("truecolor"), Some("xterm-256color")),
        ColorMode::TrueColor,
    );
}

// -- fg/bg param formatting -----------------------------------------------

#[test]
fn fg_param_truecolor() {
    assert_eq!(ColorMode::TrueColor.fg_param(255, 0, 128), "38;2;255;0;128");
}

#[test]
fn fg_param_ansi256() {
    // Pure red (255, 0, 0) → cube index 196.
    assert_eq!(ColorMode::Ansi256.fg_param(255, 0, 0), "38;5;196");
}

#[test]
fn bg_param_truecolor() {
    assert_eq!(ColorMode::TrueColor.bg_param(40, 40, 40), "48;2;40;40;40");
}

#[test]
fn bg_param_ansi256() {
    // Gray (40, 40, 40) → nearest grayscale or cube entry.
    let param = ColorMode::Ansi256.bg_param(40, 40, 40);
    assert!(param.starts_with("48;5;"), "got: {param}");
}

// -- rgb_to_ansi256 -------------------------------------------------------

#[test]
fn pure_black() {
    // (0,0,0) → cube index 16 (the 0,0,0 cube entry).
    assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
}

#[test]
fn pure_white() {
    // (255,255,255) → cube index 231 (the 5,5,5 cube entry).
    assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
}

#[test]
fn pure_red() {
    // (255,0,0) → cube index 16 + 36*5 + 6*0 + 0 = 196.
    assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
}

#[test]
fn mid_gray() {
    // (128,128,128) → should pick a grayscale index.
    let idx = rgb_to_ansi256(128, 128, 128);
    assert!((232..=255).contains(&idx), "expected grayscale, got {idx}");
}

#[test]
fn gruvbox_fg() {
    // Gruvbox dark foreground: #ebdbb2 = (235, 219, 178).
    let idx = rgb_to_ansi256(235, 219, 178);
    // Should be a warm off-white in the cube or grayscale.
    assert!(idx >= 16, "expected extended color, got {idx}");
}

// -- styled_ranges_to_escaped ---------------------------------------------

#[test]
fn ranges_truecolor_format() {
    let style = syntect::highlighting::Style {
        foreground: Color {
            r: 100,
            g: 200,
            b: 50,
            a: 255,
        },
        background: Color::BLACK,
        font_style: FontStyle::default(),
    };
    let ranges = vec![(style, "hello")];
    let out = styled_ranges_to_escaped(&ranges, false, ColorMode::TrueColor);
    assert_eq!(out, "\x1b[38;2;100;200;50mhello");
}

#[test]
fn ranges_ansi256_format() {
    let style = syntect::highlighting::Style {
        foreground: Color {
            r: 100,
            g: 200,
            b: 50,
            a: 255,
        },
        background: Color::BLACK,
        font_style: FontStyle::default(),
    };
    let ranges = vec![(style, "hello")];
    let out = styled_ranges_to_escaped(&ranges, false, ColorMode::Ansi256);
    assert!(out.contains("\x1b[38;5;"), "expected 256-color, got: {out}");
    assert!(out.ends_with("hello"));
    // Must NOT contain the 24-bit format.
    assert!(!out.contains("38;2;"));
}

#[test]
fn ranges_with_background() {
    let style = syntect::highlighting::Style {
        foreground: Color::WHITE,
        background: Color {
            r: 40,
            g: 40,
            b: 40,
            a: 255,
        },
        font_style: FontStyle::default(),
    };
    let ranges = vec![(style, "x")];
    let out = styled_ranges_to_escaped(&ranges, true, ColorMode::Ansi256);
    assert!(out.contains("48;5;"), "expected bg escape, got: {out}");
    assert!(out.contains("38;5;"), "expected fg escape, got: {out}");
}
