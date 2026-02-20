//! Theme resolution for syntax highlighting.
//!
//! Resolves a [`Theme`] from the user's configuration
//! using [`two_face`]'s bundled theme assets (curated by the `bat` project).
//!
//! [`Theme`]: syntect::highlighting::Theme

use syntect::highlighting::Theme;
use two_face::theme::{self, EmbeddedLazyThemeSet, EmbeddedThemeName};

/// The default dark theme.
const DEFAULT_DARK: EmbeddedThemeName = EmbeddedThemeName::MonokaiExtended;

/// The default light theme.
#[expect(
    dead_code,
    reason = "will be used once we detect terminal color scheme"
)]
const DEFAULT_LIGHT: EmbeddedThemeName = EmbeddedThemeName::MonokaiExtendedLight;

/// Resolve a `syntect` theme by name, or fall back to the default dark theme.
///
/// When `name` is `None`, the default dark theme is used.
///
/// The name is matched against `two_face`'s embedded theme names (e.g.
/// `"Dracula"`, `"Nord"`, `"Monokai Extended"`, `"Solarized (dark)"`).
/// If the name doesn't match any embedded theme, the default is used.
///
/// Returns a cloned `Theme` so callers can own it without lifetime ties to
/// the embedded theme set.
#[must_use]
pub fn resolve(name: Option<&str>) -> Theme {
    let themes = theme::extra();

    name.map_or_else(
        || themes[DEFAULT_DARK].clone(),
        |n| resolve_by_name(&themes, n),
    )
}

/// Look up a theme by its display name in the embedded set.
fn resolve_by_name(themes: &EmbeddedLazyThemeSet, name: &str) -> Theme {
    // Try to find an exact match against the theme's canonical name.
    for &variant in all_theme_names() {
        if variant.as_name().eq_ignore_ascii_case(name) {
            return themes[variant].clone();
        }
    }

    // No match — fall back to default.
    themes[DEFAULT_DARK].clone()
}

/// The canonical name of the default dark theme.
#[must_use]
pub fn default_theme_name() -> &'static str {
    DEFAULT_DARK.as_name()
}

/// All embedded theme variants.
#[must_use]
pub const fn all_theme_names() -> &'static [EmbeddedThemeName] {
    use EmbeddedThemeName::{
        Ansi, Base16, Base16_256, Base16EightiesDark, Base16MochaDark, Base16OceanDark,
        Base16OceanLight, ColdarkCold, ColdarkDark, DarkNeon, Dracula, Github, GruvboxDark,
        GruvboxLight, InspiredGithub, Leet, MonokaiExtended, MonokaiExtendedBright,
        MonokaiExtendedLight, MonokaiExtendedOrigin, Nord, OneHalfDark, OneHalfLight,
        SolarizedDark, SolarizedLight, SublimeSnazzy, TwoDark, Zenburn,
    };
    &[
        Ansi,
        Base16,
        Base16EightiesDark,
        Base16MochaDark,
        Base16OceanDark,
        Base16OceanLight,
        Base16_256,
        ColdarkCold,
        ColdarkDark,
        DarkNeon,
        Dracula,
        Github,
        GruvboxDark,
        GruvboxLight,
        InspiredGithub,
        Leet,
        MonokaiExtended,
        MonokaiExtendedBright,
        MonokaiExtendedLight,
        MonokaiExtendedOrigin,
        Nord,
        OneHalfDark,
        OneHalfLight,
        SolarizedDark,
        SolarizedLight,
        SublimeSnazzy,
        TwoDark,
        Zenburn,
    ]
}
