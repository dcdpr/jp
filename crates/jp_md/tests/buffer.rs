use std::fs;

use jp_md::buffer::Buffer;

#[test]
fn test_buffer_chunks() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = format!("{manifest_dir}/tests/fixtures");
    let glob_pattern = format!("{fixtures_dir}/*.md");

    let paths: Vec<_> = glob::glob(&glob_pattern)
        .expect("Failed to read glob pattern")
        .filter_map(Result::ok)
        .collect();

    assert!(!paths.is_empty(), "No fixtures found");

    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy();
        let content = fs::read_to_string(&path).expect("Failed to read file");

        // Streaming is disabled: this snapshot characterizes the buffer's
        // non-streaming block segmentation; `ParagraphChunk`s would change the
        // event sequence for long paragraphs.
        let mut buffer = Buffer::new().with_streaming_paragraphs(false);
        buffer.push(&content);

        let mut chunks = Vec::new();
        for event in &mut buffer {
            chunks.push(event);
        }
        chunks.extend(buffer.flush_events());

        insta::with_settings!({
            description => format!("Source: {}", name),
            omit_expression => true,
        }, {
            insta::assert_debug_snapshot!(name.as_ref(), chunks);
        });
    }
}
