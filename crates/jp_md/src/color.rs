//! Terminal color mode detection and RGB-to-256 conversion.
//!
//! Terminals vary in their color support: some handle 24-bit RGB escapes
//! (`\x1b[38;2;R;G;Bm`), while others (e.g. Apple Terminal.app) only
//! understand 256-color palette escapes (`\x1b[38;5;Nm`). Sending 24-bit
//! sequences to a 256-color terminal causes misparse — the extra parameters
//! get interpreted as separate SGR codes, producing garbled colors.
//!
//! This module detects the terminal's capability from environment variables
//! and provides escape sequence formatting that adapts accordingly.

use std::fmt::Write as _;

use syntect::highlighting::Color;

/// Terminal color capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// 24-bit RGB colors (`\x1b[38;2;R;G;Bm`).
    #[default]
    TrueColor,

    /// 256-color palette (`\x1b[38;5;Nm`).
    Ansi256,

    /// No color escapes at all.
    Plain,
}

impl ColorMode {
    /// Detect terminal color capability from environment variable values.
    ///
    /// Checks `COLORTERM` first — terminals that support 24-bit color
    /// advertise it there. Falls back to `TERM` for 256-color detection.
    /// When neither variable provides a clear signal, defaults to
    /// [`TrueColor`](Self::TrueColor).
    #[must_use]
    pub fn detect(colorterm: Option<&str>, term: Option<&str>) -> Self {
        if let Some(ct) = colorterm
            && (ct.eq_ignore_ascii_case("truecolor") || ct.eq_ignore_ascii_case("24bit"))
        {
            return Self::TrueColor;
        }

        if let Some(t) = term
            && t.contains("256color")
        {
            return Self::Ansi256;
        }

        Self::TrueColor
    }

    /// Format a foreground color SGR parameter (without `\x1b[` / `m`).
    ///
    /// e.g. `"38;2;255;0;0"` (truecolor) or `"38;5;196"` (256-color).
    #[must_use]
    pub fn fg_param(self, r: u8, g: u8, b: u8) -> String {
        match self {
            Self::TrueColor => format!("38;2;{r};{g};{b}"),
            Self::Ansi256 => format!("38;5;{}", rgb_to_ansi256(r, g, b)),
            Self::Plain => String::new(),
        }
    }

    /// Format a background color SGR parameter (without `\x1b[` / `m`).
    #[must_use]
    pub fn bg_param(self, r: u8, g: u8, b: u8) -> String {
        match self {
            Self::TrueColor => format!("48;2;{r};{g};{b}"),
            Self::Ansi256 => format!("48;5;{}", rgb_to_ansi256(r, g, b)),
            Self::Plain => String::new(),
        }
    }

    /// Format a complete foreground color escape sequence.
    #[must_use]
    pub fn fg_escape(self, r: u8, g: u8, b: u8) -> String {
        if self == Self::Plain {
            return String::new();
        }
        format!("\x1b[{}m", self.fg_param(r, g, b))
    }

    /// Format a complete background color escape sequence.
    #[must_use]
    pub fn bg_escape(self, r: u8, g: u8, b: u8) -> String {
        if self == Self::Plain {
            return String::new();
        }
        format!("\x1b[{}m", self.bg_param(r, g, b))
    }
}

/// Format styled text ranges as terminal escape sequences, respecting the
/// terminal's color capability.
///
/// Drop-in replacement for `syntect::util::as_24_bit_terminal_escaped`.
#[must_use]
pub fn styled_ranges_to_escaped(
    ranges: &[(syntect::highlighting::Style, &str)],
    bg: bool,
    mode: ColorMode,
) -> String {
    if mode == ColorMode::Plain {
        return ranges.iter().map(|(_, text)| *text).collect();
    }

    let mut s = String::new();
    for &(ref style, text) in ranges {
        if bg {
            let Color { r, g, b, .. } = style.background;
            let _ = write!(s, "\x1b[{}m", mode.bg_param(r, g, b));
        }
        let fg = blend_fg(style.foreground, style.background);
        let _ = write!(s, "\x1b[{}m{text}", mode.fg_param(fg.r, fg.g, fg.b));
    }
    s
}

/// Blend foreground alpha against background.
///
/// Replicates `syntect::util::blend_fg_color` (which is not public).
#[expect(clippy::cast_possible_truncation)]
const fn blend_fg(fg: Color, bg: Color) -> Color {
    if fg.a == 0xff {
        return fg;
    }
    let a = fg.a as u32;
    Color {
        r: ((fg.r as u32 * a + bg.r as u32 * (255 - a)) / 255) as u8,
        g: ((fg.g as u32 * a + bg.g as u32 * (255 - a)) / 255) as u8,
        b: ((fg.b as u32 * a + bg.b as u32 * (255 - a)) / 255) as u8,
        a: 255,
    }
}

// -- RGB to 256-color conversion ------------------------------------------

/// The 6 values along each axis of the 6×6×6 color cube (indices 16–231).
const CUBE_STEPS: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Convert an RGB color to the nearest 256-color palette index.
///
/// Compares the input against both the 6×6×6 color cube (indices 16–231)
/// and the 24-step grayscale ramp (indices 232–255), returning whichever
/// is closer in squared Euclidean distance.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let ci = nearest_cube_axis(r);
    let cj = nearest_cube_axis(g);
    let ck = nearest_cube_axis(b);
    let cube_idx = 16 + 36 * ci + 6 * cj + ck;
    let cube_dist = sq_dist(
        r,
        g,
        b,
        CUBE_STEPS[ci as usize],
        CUBE_STEPS[cj as usize],
        CUBE_STEPS[ck as usize],
    );

    // Grayscale ramp: index 232 = level 8, 233 = 18, ..., 255 = 238 (step 10).
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let gray_idx: u8 = if avg < 4 {
        232
    } else if avg > 243 {
        255
    } else {
        // Round to nearest step.
        ((avg - 4) / 10) as u8 + 232
    };
    let gray_level = 8 + 10 * (i32::from(gray_idx) - 232);
    let gl = gray_level as u8;
    let gray_dist = sq_dist(r, g, b, gl, gl, gl);

    if gray_dist < cube_dist {
        gray_idx
    } else {
        cube_idx
    }
}

/// Find the nearest index (0–5) in the cube axis for a color component.
#[expect(clippy::cast_possible_truncation)]
fn nearest_cube_axis(v: u8) -> u8 {
    let mut best = 0_u8;
    let mut best_d = u16::MAX;
    for (i, &step) in CUBE_STEPS.iter().enumerate() {
        let d = (i16::from(v) - i16::from(step)).unsigned_abs();
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
}

/// Squared Euclidean distance between two RGB colors.
#[expect(clippy::cast_sign_loss)]
const fn sq_dist(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

#[cfg(test)]
#[path = "color_tests.rs"]
mod tests;
