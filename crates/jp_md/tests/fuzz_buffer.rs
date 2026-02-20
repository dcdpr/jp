use jp_md::buffer::Buffer;
use proptest::prelude::*;

fn run_fuzz_test(original: &str, chunks: Vec<&str>) {
    let mut buffer = Buffer::new();
    let mut output = Vec::new();

    for chunk in chunks {
        buffer.push(chunk);
        for event in &mut buffer {
            output.push(event);
        }
    }

    if let Some(flushed) = buffer.flush() {
        output.push(flushed);
    }

    // 1. Run full text through buffer -> expected
    // 2. Run chunks through buffer -> actual
    // 3. Assert equality

    let mut expected_buffer = Buffer::from(original);
    let mut expected = Vec::new();
    for event in &mut expected_buffer {
        expected.push(event);
    }
    if let Some(flushed) = expected_buffer.flush() {
        expected.push(flushed);
    }

    assert_eq!(output, expected, "Failed with fragmented input");
}

// Strategy to split a string into random chunks
fn chunks_strategy(text: String) -> impl Strategy<Value = Vec<String>> {
    assert!(!text.is_empty(), "Cannot split empty string");

    let char_indices: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let char_count = char_indices.len();

    // Strategy for the NUMBER of split points.
    //
    // We mix different "densities" to ensure we get large chunks (few splits)
    // as well as small chunks (many splits).
    let split_count_strategy = prop_oneof![
        // 20% chance: 0 or 1 split (1 or 2 chunks total).
        // This tests "almost whole" inputs.
        20 => 0..=1usize,

        // 60% chance: 1 to 10 splits (2 to 11 chunks).
        // This simulates reasonable streaming chunks (e.g. network packets).
        60 => 1..=std::cmp::min(10, char_count),

        // 20% chance: High fragmentation (up to 1 split per char).
        // This tests edge cases with tiny chunks.
        20 => std::cmp::min(10, char_count)..=char_count,
    ];

    split_count_strategy
        .prop_flat_map(move |count| {
            let char_indices = char_indices.clone();
            let text = text.clone();

            // We pick indices from the list of valid char indices
            proptest::collection::vec(0..char_count, count).prop_map(move |index_indices| {
                // Map back to byte indices
                let mut byte_indices: Vec<usize> =
                    index_indices.iter().map(|&i| char_indices[i]).collect();

                byte_indices.push(0);
                byte_indices.push(text.len());
                byte_indices.sort_unstable();
                byte_indices.dedup();

                let mut chunks = Vec::new();
                for window in byte_indices.windows(2) {
                    let start = window[0];
                    let end = window[1];
                    if start < end {
                        chunks.push(text[start..end].to_string());
                    }
                }

                chunks
            })
        })
        .boxed()
}

fn fixture_content_strategy() -> impl Strategy<Value = (String, String)> {
    let glob_pattern = format!("{}/tests/fixtures/*.md", env!("CARGO_MANIFEST_DIR"));
    let paths: Vec<_> = glob::glob(&glob_pattern)
        .expect("Failed to read glob pattern")
        .filter_map(Result::ok)
        .collect();

    assert!(!paths.is_empty(), "No fixtures found in {glob_pattern}");

    let path_indices = 0..paths.len();

    path_indices.prop_map(move |idx| {
        let path = &paths[idx];
        let content = std::fs::read_to_string(path).expect("Failed to read file");
        (
            path.file_name().unwrap().to_string_lossy().to_string(),
            content,
        )
    })
}

// Better approach with prop_flat_map
fn fixture_and_chunks_strategy() -> impl Strategy<Value = (String, Vec<String>)> {
    fixture_content_strategy().prop_flat_map(|(filename, content)| {
        let chunks = chunks_strategy(content);
        chunks.prop_map(move |c| (filename.clone(), c))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn fuzz_chunked_processing((_filename, chunks) in fixture_and_chunks_strategy()) {
        let original: String = chunks.join("");
        let chunk_slices: Vec<&str> = chunks.iter().map(String::as_str).collect();

        // println!("Testing file: {_filename}"); // Noisy, but useful for debugging if needed

        run_fuzz_test(&original, chunk_slices);
    }
}
