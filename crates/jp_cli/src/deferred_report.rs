//! Reports printed on stderr when the run ends.
//!
//! A command's own output comes first; what JP has to say about the workspace
//! the command ran in follows it, whichever way the run ends.

use jp_printer::Printer;
use tracing::warn;

/// A report printed on stderr when dropped.
///
/// The report is printed past `--quiet`: it is about the user's data, not
/// chrome about the run.
pub(crate) struct DeferredReport {
    printer: Printer,
    lines: Vec<String>,
    /// Whether `lines` are JSON records, printed without the printer's
    /// `{"message": ...}` wrapping.
    raw: bool,
}

impl DeferredReport {
    /// Prepare a report for `printer`'s output format.
    ///
    /// Under the text formats the report is what `text` renders, if anything.
    /// Under the JSON formats it is the records `json` renders, one per line,
    /// indented when `json` is passed `true`.
    /// A report that fails to serialize falls back to `text`, which the printer
    /// wraps in a JSON record, so the stream stays parseable either way.
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
