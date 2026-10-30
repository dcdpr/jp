//! Logical styling primitives for markdown elements.

/// Logical terminal color: an ANSI 256-color palette index or 24-bit RGB.
///
/// Owned by `jp_md` so SGR escape construction stays inside the renderer crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// ANSI 256-color palette index (`0`–`255`).
    Ansi256(u8),
    /// 24-bit RGB color.
    Rgb {
        /// Red channel.
        r: u8,
        /// Green channel.
        g: u8,
        /// Blue channel.
        b: u8,
    },
}

impl Color {
    /// Return the SGR foreground parameter string (without `\x1b[` and `m`).
    #[must_use]
    pub fn fg_param(self) -> String {
        match self {
            Self::Ansi256(n) => format!("38;5;{n}"),
            Self::Rgb { r, g, b } => format!("38;2;{r};{g};{b}"),
        }
    }

    /// Return the SGR background parameter string (without `\x1b[` and `m`).
    #[must_use]
    pub fn bg_param(self) -> String {
        match self {
            Self::Ansi256(n) => format!("48;5;{n}"),
            Self::Rgb { r, g, b } => format!("48;2;{r};{g};{b}"),
        }
    }
}

/// Per-element visual style overrides.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    /// Foreground color override.
    pub fg: Option<Color>,
    /// Background color override.
    pub bg: Option<Color>,
}

/// Override stylesheet for markdown elements emitted by the terminal renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkdownStyleSheet {
    /// Style applied to inline and display math spans (`$..$`, `$$..$$`).
    pub math: Style,
}
