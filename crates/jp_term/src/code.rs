use syntect::{
    easy::HighlightLines,
    highlighting::ThemeSet,
    parsing::SyntaxSet,
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};

pub struct Config {
    /// The language sytax to use for syntax highlighting.
    pub language: Option<String>,

    /// The color theme to use for syntax highlighting.
    ///
    /// If `None`, no coloring will be used.
    pub theme: Option<String>,
}

/// Format a code block.
///
/// Returns `true` if the code block was formatted successfully, and `false`
/// if the code block was not formatted because the language was unknown.
///
/// # Errors
///
/// Returns an error if the code block could not be formatted.
pub fn format(content: &str, buf: &mut String, config: &Config) -> Result<bool, syntect::Error> {
    let ss = SyntaxSet::load_defaults_newlines();

    let syntax = match config.language.as_deref() {
        Some(lang) => match ss.find_syntax_by_token(lang) {
            Some(s) => s,
            None => return Ok(false),
        },
        None => ss.find_syntax_plain_text(),
    };

    let Some(theme_name) = config.theme.as_deref() else {
        buf.push_str(content);
        return Ok(true);
    };

    let ts = ThemeSet::load_defaults();
    let Some(theme) = ts.themes.get(theme_name) else {
        buf.push_str(content);
        return Ok(true);
    };

    let mut h = HighlightLines::new(syntax, theme);
    for line in LinesWithEndings::from(content) {
        let ranges = h.highlight_line(line, &ss)?;
        let escaped = as_24_bit_terminal_escaped(&ranges, true);
        buf.push_str(&escaped);
    }
    buf.push_str("\x1b[0m");

    Ok(true)
}
