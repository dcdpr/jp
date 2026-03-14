//! Structured output rendering for the query stream pipeline.
//!
//! Renders streamed JSON chunks as a fenced code block. No markdown
//! parsing, no typewriter effect — just raw JSON in a code fence.

use std::sync::Arc;

use jp_conversation::event::ChatResponse;
use jp_printer::Printer;
use serde_json::Value;

/// Renders `ChatResponse::Structured` events to the terminal as a fenced
/// JSON code block.
///
/// ````text
/// ```json
/// {"name": "Alice"}
/// ```
/// ````
pub struct StructuredRenderer {
    printer: Arc<Printer>,
    started: bool,
}

impl StructuredRenderer {
    pub fn new(printer: Arc<Printer>) -> Self {
        Self {
            printer,
            started: false,
        }
    }

    /// Render a single structured chunk.
    ///
    /// On the first chunk, emits the opening code fence. Subsequent chunks
    /// are appended directly.
    pub fn render_chunk(&mut self, response: &ChatResponse) {
        let ChatResponse::Structured { data } = response else {
            return;
        };
        let Value::String(chunk) = data else {
            return;
        };

        if !self.started {
            self.printer.print("```json\n");
            self.started = true;
        }

        self.printer.print(chunk);
    }

    /// Close the fenced code block, if one was opened.
    pub fn flush(&mut self) {
        if self.started {
            self.printer.print("\n```\n");
            self.started = false;
        }
    }

    /// Reset the renderer state, discarding tracking of whether a code
    /// fence is open.
    pub fn reset(&mut self) {
        self.started = false;
    }
}

#[cfg(test)]
#[path = "structured_renderer_tests.rs"]
mod tests;
