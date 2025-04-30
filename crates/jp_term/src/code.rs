use bat::PrettyPrinter;

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
pub fn format(content: &str, buf: &mut String, config: &Config) -> Result<bool, bat::error::Error> {
    let mut printer = PrettyPrinter::new();
    printer
        .input_from_bytes(content.as_bytes())
        .line_numbers(false);

    if let Some(language) = config.language.as_deref() {
        printer.language(language);
    }

    match config.theme.as_deref() {
        Some(theme) => printer.theme(theme),
        None => printer.colored_output(false),
    };

    match printer.print_with_writer(Some(buf)) {
        Ok(_) => Ok(true),
        Err(bat::error::Error::UnknownSyntax(_)) => Ok(false),
        Err(e) => Err(e),
    }
}
