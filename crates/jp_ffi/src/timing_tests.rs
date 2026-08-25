use std::ffi::CStr;

use super::*;

/// A timings payload, character for character.
///
/// Pinned here and in `apps/macos/Tests/WorkspaceReaderTests.swift`, which
/// decodes this exact string.
/// Nothing else checks that the two sides agree on the shape: if one of these
/// two literals is edited alone, the other test is what says so.
const TIMINGS_JSON: &str = r#"[{"name":"storage.read","duration_ms":1.234},{"name":"deserialize","duration_ms":84.219},{"name":"serialize","duration_ms":3.0}]"#;

#[test]
fn spans_serialize_in_the_order_they_were_recorded() {
    let mut timings = Timings::default();
    timings.record("storage.read", Duration::from_micros(1_234));
    timings.record("deserialize", Duration::from_micros(84_219));
    timings.record("serialize", Duration::from_millis(3));

    assert_eq!(
        timings.to_c_string().unwrap().to_str().unwrap(),
        TIMINGS_JSON
    );
}

/// A call that failed before doing any of the work it measures still reports an
/// array, so the caller decodes one shape rather than two.
#[test]
fn no_spans_serialize_as_an_empty_array() {
    assert_eq!(
        Timings::default().to_c_string().unwrap().to_str().unwrap(),
        "[]"
    );
}

/// Sub-microsecond work is rounded, not truncated to zero: a span that reports
/// `0` is indistinguishable from one that never ran.
#[test]
fn durations_round_to_the_microsecond() {
    let mut timings = Timings::default();
    timings.record("nanoseconds", Duration::from_nanos(1_499));

    assert_eq!(
        timings.to_c_string().unwrap().to_str().unwrap(),
        r#"[{"name":"nanoseconds","duration_ms":0.001}]"#
    );
}

#[test]
fn measure_records_the_work_it_wraps() {
    let mut timings = Timings::default();
    let value = timings.measure("work", || 7);

    assert_eq!(value, 7);
    assert_eq!(timings.spans.len(), 1);
    assert_eq!(timings.spans[0].name, "work");
}

#[test]
fn publishing_to_a_null_slot_is_a_no_op() {
    // SAFETY: null is the one slot value the contract admits without a
    // variable behind it, and `publish` checks for it before writing.
    unsafe { publish(ptr::null_mut(), &Timings::default()) };
}

#[test]
fn publishing_fills_the_slot_with_a_string_the_caller_frees() {
    let mut timings = Timings::default();
    timings.record("serialize", Duration::from_millis(3));

    let mut slot: *mut c_char = ptr::null_mut();

    // SAFETY: `slot` is a live, writable `*mut c_char` that outlives the call.
    unsafe { publish(&raw mut slot, &timings) };

    assert!(!slot.is_null());

    // SAFETY: `slot` was just written with a `CString::into_raw` pointer, so it
    // is NUL-terminated and reclaiming it as a `CString` pairs the allocation
    // with its original allocator. It is not used after being freed.
    unsafe {
        assert_eq!(
            CStr::from_ptr(slot).to_str().unwrap(),
            r#"[{"name":"serialize","duration_ms":3.0}]"#
        );
        drop(CString::from_raw(slot));
    }
}
