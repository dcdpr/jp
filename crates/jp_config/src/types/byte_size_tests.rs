use super::*;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_bare_byte_count() {
    assert_eq!("1024".parse::<ByteSize>().unwrap().as_bytes(), 1024);
    assert_eq!("0".parse::<ByteSize>().unwrap().as_bytes(), 0);
}

#[test]
fn parses_binary_units() {
    // Unit suffixes are binary, so `1MB` is 1048576 rather than 1000000.
    assert_eq!("1KB".parse::<ByteSize>().unwrap().as_bytes(), 1024);
    assert_eq!("1MB".parse::<ByteSize>().unwrap().as_bytes(), 1_048_576);
    assert_eq!("1GB".parse::<ByteSize>().unwrap().as_bytes(), 1_073_741_824);
    assert_eq!("256KB".parse::<ByteSize>().unwrap().as_bytes(), 262_144);
}

#[test]
fn parses_unit_spelling_variants() {
    // `KiB` is an explicit spelling of the same binary value as `KB`, and
    // casing and internal spacing are not significant.
    for input in ["1KB", "1kb", "1KiB", "1kib", "1 KB", " 1MB ".trim(), "1k"] {
        let parsed = input.parse::<ByteSize>().unwrap();
        assert!(
            parsed.as_bytes() == 1024 || parsed.as_bytes() == 1_048_576,
            "`{input}` parsed to {} bytes",
            parsed.as_bytes()
        );
    }

    assert_eq!("1KiB".parse::<ByteSize>().unwrap(), "1KB".parse().unwrap());
    assert_eq!("4MiB".parse::<ByteSize>().unwrap().as_bytes(), 4_194_304);
}

#[test]
fn parses_explicit_byte_suffix() {
    assert_eq!("512B".parse::<ByteSize>().unwrap().as_bytes(), 512);
}

#[test]
fn rejects_unknown_unit() {
    let err = "1TB".parse::<ByteSize>().unwrap_err().to_string();
    assert_eq!(
        err,
        "invalid size `1TB`: unknown unit `tb` (expected B, KB, MB, or GB)"
    );
}

#[test]
fn rejects_missing_byte_count() {
    let err = "MB".parse::<ByteSize>().unwrap_err().to_string();
    assert_eq!(err, "invalid size `MB`: expected a leading byte count");
}

#[test]
fn rejects_overflow() {
    let err = "99999999999999999999GB"
        .parse::<ByteSize>()
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "invalid size `99999999999999999999GB`: byte count out of range"
    );

    let err = "17179869184GB".parse::<ByteSize>().unwrap_err().to_string();
    assert_eq!(err, "invalid size `17179869184GB`: value overflows");
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[test]
fn display_uses_largest_evenly_dividing_unit() {
    assert_eq!(ByteSize::from_bytes(1_048_576).to_string(), "1MB");
    assert_eq!(ByteSize::from_bytes(262_144).to_string(), "256KB");
    assert_eq!(ByteSize::from_bytes(1_073_741_824).to_string(), "1GB");
    assert_eq!(ByteSize::from_bytes(512).to_string(), "512");
    assert_eq!(ByteSize::from_bytes(0).to_string(), "0");
}

#[test]
fn display_round_trips_exactly() {
    // The serialized form is `Display`, so any value must parse back to itself
    // even when no unit divides it evenly.
    for bytes in [0, 1, 512, 1024, 1500, 1_048_576, 10_900_000, 1_073_741_825] {
        let size = ByteSize::from_bytes(bytes);
        let parsed: ByteSize = size.to_string().parse().unwrap();
        assert_eq!(parsed, size, "{bytes} did not round-trip");
    }
}

#[test]
fn human_is_approximate_with_one_decimal() {
    assert_eq!(ByteSize::from_bytes(10_900_000).human(), "10.3 MB");
    assert_eq!(ByteSize::from_bytes(1_048_576).human(), "1.0 MB");
    assert_eq!(ByteSize::from_bytes(1536).human(), "1.5 KB");
    assert_eq!(ByteSize::from_bytes(512).human(), "512 B");
}

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

#[test]
fn serializes_as_a_string() {
    let json = serde_json::to_value(ByteSize::from_bytes(1_048_576)).unwrap();
    assert_eq!(json, serde_json::json!("1MB"));
}

#[test]
fn deserializes_from_string_or_integer() {
    let from_string: ByteSize = serde_json::from_value(serde_json::json!("1MB")).unwrap();
    let from_integer: ByteSize = serde_json::from_value(serde_json::json!(1_048_576)).unwrap();

    assert_eq!(from_string, from_integer);
    assert_eq!(from_string.as_bytes(), 1_048_576);
}

#[test]
fn rejects_negative_integer() {
    let err = serde_json::from_value::<ByteSize>(serde_json::json!(-1)).unwrap_err();
    assert!(
        err.to_string().contains("size must be non-negative"),
        "unexpected error: {err}"
    );
}
