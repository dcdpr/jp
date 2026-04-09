//! Structured output rendering for the query stream pipeline and conversation
//! replay.
//!
//! Renders structured JSON as a fenced code block. In the live-stream path,
//! chunks arrive as `Value::String` fragments. In the replay path, the
//! complete parsed value is pretty-printed.

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
            Value::String(chunk) => self.printer.print(chunk),
            other => {
                let text =
                    serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
                self.printer.print(text);
            }
        }
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
#[path = "structured_tests.rs"]
mod tests;
