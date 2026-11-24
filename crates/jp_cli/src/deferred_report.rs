//! End-of-run reports printed to stderr after command output.

use jp_printer::Printer;
use tracing::warn;

/// A report printed to stderr when dropped, bypassing `--quiet` so data
/// warnings are never hidden.
pub(crate) struct DeferredReport {
    printer: Printer,
    lines: Vec<String>,
    /// Whether `lines` are JSON records, printed without the printer's
    /// `{"message": ...}` wrapping.
    raw: bool,
}

impl DeferredReport {
    /// Build a deferred report for `printer`'s output format.
    ///
    /// In text formats, prints the string returned by `text` (if `Some`).
    /// In JSON formats, prints each pre-serialized record returned by `json`
    /// (passing `true` when pretty-printing is enabled).
    /// If JSON serialization fails, logs a warning and falls back to `text`
    /// wrapped in the printer's standard JSON message envelope.
    pub(crate) fn new(
        printer: Printer,
        text: impl FnOnce() -> Option<String>,
        json: impl FnOnce(bool) -> serde_json::Result<Vec<String>>,
    ) -> Self {
        let format = printer.format();
        let records = format.is_json().then(|| json(format.is_json_pretty()));

        let (lines, raw) = match records {
            Some(Ok(records)) => (records, true),
            Some(Err(error)) => {
                warn!(%error, "Failed to serialize a report.");
                (text().into_iter().collect(), false)
            }
            None => (text().into_iter().collect(), false),
        };

        Self {
            printer,
            lines,
            raw,
        }
    }
}

impl Drop for DeferredReport {
    fn drop(&mut self) {
        if self.lines.is_empty() {
            return;
        }

        for line in self.lines.drain(..) {
            if self.raw {
                self.printer.error_println_raw(line);
            } else {
                self.printer.error_println(line);
            }
        }
        self.printer.flush();
    }
}
