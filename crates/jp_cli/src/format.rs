pub(crate) mod conversation;
pub(crate) mod datetime;

use jp_config::types::color::Color;

/// Convert a [`Color`] to an SGR background parameter string.
pub(crate) fn color_to_bg_param(color: Color) -> String {
    match color {
        Color::Ansi256(n) => format!("48;5;{n}"),
        Color::Rgb { r, g, b } => format!("48;2;{r};{g};{b}"),
    }
}
