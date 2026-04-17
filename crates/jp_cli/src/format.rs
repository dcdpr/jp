pub(crate) mod conversation;
pub(crate) mod datetime;

use jp_config::{style::markdown::ColorModeConfig, types::color::Color};
use jp_md::color::ColorMode;

/// Convert a [`Color`] to an SGR background parameter string,
/// respecting the terminal's color capability.
pub(crate) fn color_to_bg_param(color: Color, color_mode: ColorMode) -> String {
    match color {
        Color::Ansi256(n) => format!("48;5;{n}"),
        Color::Rgb { r, g, b } => color_mode.bg_param(r, g, b),
    }
}

/// Resolve [`ColorModeConfig`] to a concrete [`ColorMode`].
///
/// `Auto` reads `COLORTERM` and `TERM` from the environment.
pub(crate) fn resolve_color_mode(cfg: ColorModeConfig) -> ColorMode {
    match cfg {
        ColorModeConfig::Auto => ColorMode::detect(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ),
        ColorModeConfig::Truecolor => ColorMode::TrueColor,
        ColorModeConfig::Ansi256 => ColorMode::Ansi256,
        ColorModeConfig::Plain => ColorMode::Plain,
    }
}
