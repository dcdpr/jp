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
