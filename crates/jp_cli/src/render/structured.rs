//! Structured output rendering for the query stream pipeline and conversation
//! replay.
//!
//! Renders structured JSON as a fenced code block.
//! In the live-stream path, chunks arrive as `Value::String` fragments.
//! In the replay path, the complete parsed value is pretty-printed.
//! Structured output is model output, so it is shown the way `style.sanitize`
//! asks, and keeps no escape sequences under `strip`.

use std::{fmt::Write as _, sync::Arc};

use jp_conversation::event::ChatResponse;
use jp_printer::Printer;
use jp_term::sanitize::{ContentClass, ContentWriter, SanitizeMode, sanitize_str};
use serde_json::Value;

/// Renders `ChatResponse::Structured` events to the terminal as a fenced JSON
/// code block.
///
/// ````text
/// ```json
/// {"name": "Alice"}
/// ```
/// ````
pub struct StructuredRenderer {
    printer: Arc<Printer>,
    started: bool,

    /// How `style.sanitize` has model output shown.
    mode: SanitizeMode,

    /// Filters streamed fragments, holding a sequence split between two of them
    /// until the rest arrives.
    ///
    /// Boxed because the parser state is a few hundred bytes, and this renderer
    /// lives inside every turn loop's future.
    filter: Box<ContentWriter<String>>,
}

impl StructuredRenderer {
    pub fn new(printer: Arc<Printer>, mode: SanitizeMode) -> Self {
        Self {
            printer,
            started: false,
            mode,
            filter: model_output_filter(mode),
        }
    }

    /// Render a single structured chunk.
    ///
    /// On the first chunk, emits the opening code fence.
    /// Subsequent chunks are appended directly.
    ///
    /// - `Value::String` is printed as raw text (streaming fragments).
    /// - Any other variant is pretty-printed as complete JSON.
    pub fn render_chunk(&mut self, response: &ChatResponse) {
        let ChatResponse::Structured { data } = response else {
            return;
        };

        if !self.started {
            self.printer.print("```json\n");
            self.started = true;
        }

        match data {
            Value::String(chunk) => {
                // Writing to a `String` is infallible.
                let _ = self.filter.write_str(chunk);
                self.print_filtered();
            }
            other => {
                // JSON escapes C0 characters in a string, but leaves DEL and C1
                // as they are.
                let text =
                    serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
                self.printer
                    .print(sanitize_str(&text, ContentClass::ModelOutput, self.mode));
            }
        }
    }

    /// Close the fenced code block, if one was opened.
    ///
    /// A sequence the last fragment left unfinished is dropped ahead of the
    /// fence, so it cannot swallow it.
    pub fn flush(&mut self) {
        if self.started {
            // Writing to a `String` is infallible.
            let _ = self.filter.finish();
            self.print_filtered();
            self.printer.print("\n```\n");
            self.started = false;
        }
    }

    /// Reset the renderer state, discarding tracking of whether a code fence is
    /// open.
    ///
    /// A sequence the interrupted stream left unfinished is discarded with it.
    pub fn reset(&mut self) {
        self.started = false;
        self.filter = model_output_filter(self.mode);
    }

    /// Print what the filter has let through so far.
    fn print_filtered(&mut self) {
        let shown = std::mem::take(self.filter.get_mut());
        if !shown.is_empty() {
            self.printer.print(shown);
        }
    }
}

/// A fresh filter for structured output shown under `mode`.
fn model_output_filter(mode: SanitizeMode) -> Box<ContentWriter<String>> {
    Box::new(ContentWriter::new(
        String::new(),
        ContentClass::ModelOutput,
        mode,
    ))
}

#[cfg(test)]
#[path = "structured_tests.rs"]
mod tests;
