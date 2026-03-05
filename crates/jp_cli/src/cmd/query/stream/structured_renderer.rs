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
mod tests {
    use jp_printer::OutputFormat;

    use super::*;

    fn create_renderer() -> (StructuredRenderer, jp_printer::SharedBuffer, Printer) {
        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let renderer = StructuredRenderer::new(Arc::new(printer.clone()));
        (renderer, out, printer)
    }

    #[test]
    fn renders_json_in_code_fence() {
        let (mut renderer, out, printer) = create_renderer();

        renderer.render_chunk(&ChatResponse::Structured {
            data: Value::String("{\"name\"".into()),
        });
        renderer.render_chunk(&ChatResponse::Structured {
            data: Value::String(": \"Alice\"}".into()),
        });
        renderer.flush();
        printer.flush();

        assert_eq!(*out.lock(), "```json\n{\"name\": \"Alice\"}\n```\n");
    }

    #[test]
    fn flush_without_chunks_is_noop() {
        let (mut renderer, out, printer) = create_renderer();

        renderer.flush();
        printer.flush();

        assert_eq!(*out.lock(), "");
    }

    #[test]
    fn ignores_non_structured_variants() {
        let (mut renderer, out, printer) = create_renderer();

        renderer.render_chunk(&ChatResponse::Message {
            message: "hello".into(),
        });
        renderer.flush();
        printer.flush();

        assert_eq!(*out.lock(), "");
    }

    #[test]
    fn ignores_non_string_data() {
        let (mut renderer, out, printer) = create_renderer();

        renderer.render_chunk(&ChatResponse::Structured {
            data: serde_json::json!({"already": "parsed"}),
        });
        renderer.flush();
        printer.flush();

        assert_eq!(*out.lock(), "");
    }

    #[test]
    fn reset_allows_new_code_fence() {
        let (mut renderer, out, printer) = create_renderer();

        renderer.render_chunk(&ChatResponse::Structured {
            data: Value::String("{}".into()),
        });
        renderer.flush();

        // Reset and render again — should produce a second code fence
        renderer.reset();
        renderer.render_chunk(&ChatResponse::Structured {
            data: Value::String("[1,2]".into()),
        });
        renderer.flush();
        printer.flush();

        assert_eq!(*out.lock(), "```json\n{}\n```\n```json\n[1,2]\n```\n");
    }
}
