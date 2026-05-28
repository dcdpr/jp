use super::*;

const FIXTURE: &str = "\
Sampling process 12345 ...
Call graph:
    2272 Thread_23977072   DispatchQueue_1: com.apple.main-thread  (serial)
    + 2272 _RNvCsh5KVlJR8TJf_6jp_cli3run  (in jp) + 6928  [0x10555ed64]
    +   2272 _RNvCsh5KVlJR8TJf_6jp_cli9run_inner  (in jp) + 17712  [0x105564e64]
    +     1129 _RINvXs7_NtCsaqd1S84YXFC_15jp_conversation6stream18ConversationStream6extend  (in \
                       jp) + 232  [0x1055eadf0]
    +       769 _RNvXsg_CseGNDgNwdsbj_9jp_config16PartialAppConfig5clone  (in jp) + 520  \
                       [0x10562567c]
    2272 Thread_23977077: jp-worker
    + 2272 _pthread_start  (in libsystem_pthread.dylib) + 136
Total number in stack ...
";

#[test]
fn parse_splits_into_threads() {
    let threads = parse(FIXTURE);
    assert_eq!(threads.len(), 2);
    assert!(threads[0].header.starts_with("Thread_23977072"));
    assert!(threads[1].header.starts_with("Thread_23977077"));
}

#[test]
fn parse_extracts_sample_counts() {
    let threads = parse(FIXTURE);
    let frames = &threads[0].frames;
    // Fixture has four frames: run, run_inner, extend, clone.
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0].samples, 2272);
    assert_eq!(frames[1].samples, 2272);
    assert_eq!(frames[2].samples, 1129);
    assert_eq!(frames[3].samples, 769);
}

#[test]
fn parse_demangles_rust_symbols() {
    let threads = parse(FIXTURE);
    let frames = &threads[0].frames;
    // Demangled, with hash suffix stripped via `{:#}`.
    assert_eq!(frames[0].symbol, "jp_cli::run");
    assert!(
        frames[2].symbol.contains("ConversationStream"),
        "expected demangled ConversationStream::extend, got {:?}",
        frames[2].symbol
    );
}

#[test]
fn parse_passes_through_unmangled_symbols() {
    let threads = parse(FIXTURE);
    let frame = &threads[1].frames[0];
    assert_eq!(frame.symbol, "_pthread_start");
}

#[test]
fn parse_stops_at_total_number_in_stack() {
    // Anything after the trailer line is ignored.
    let extra = format!("{FIXTURE}\n    9999 should_be_ignored  (in jp) + 0");
    let threads = parse(&extra);
    assert_eq!(threads.len(), 2);
}

#[test]
fn aggregate_by_symbol_sums_duplicates() {
    let thread = Thread {
        header: "Thread_test".into(),
        frames: vec![
            Frame {
                depth: 0,
                samples: 10,
                symbol: "foo".into(),
            },
            Frame {
                depth: 1,
                samples: 5,
                symbol: "bar".into(),
            },
            Frame {
                depth: 1,
                samples: 7,
                symbol: "foo".into(),
            },
        ],
    };
    let agg = thread.aggregate_by_symbol();
    assert_eq!(agg, vec![("foo".into(), 17), ("bar".into(), 5)]);
}
