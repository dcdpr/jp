use camino::Utf8Path;

use super::*;

/// `ENOSPC` on every Unix platform this crate targets.
///
/// Building the error from the raw code keeps the classification tests honest:
/// they run against what the kernel actually delivers, not against an error
/// synthesized from the very [`io::ErrorKind`] the classifier reads back.
#[cfg(unix)]
const ENOSPC: i32 = 28;

/// An out-of-space error as the operating system reports it.
#[cfg(unix)]
fn os_disk_full() -> io::Error {
    io::Error::from_raw_os_error(ENOSPC)
}

#[cfg(not(unix))]
fn os_disk_full() -> io::Error {
    io::Error::from(io::ErrorKind::StorageFull)
}

#[test]
fn write_failed_classifies_full_disk_as_out_of_space() {
    let error = Error::write_failed(Utf8Path::new("/data/conv/events.json"), os_disk_full());

    assert!(matches!(error, Error::OutOfSpace { .. }));
    assert_eq!(
        error.to_string(),
        "no space left on device while writing /data/conv/events.json"
    );
}

#[test]
fn write_failed_classifies_other_errors_as_write_failed() {
    let error = Error::write_failed(
        Utf8Path::new("/data/conv/events.json"),
        io::Error::from(io::ErrorKind::PermissionDenied),
    );

    assert!(matches!(error, Error::WriteFailed { .. }));
    assert_eq!(error.to_string(), "failed to write /data/conv/events.json");
}

#[test]
fn write_failed_keeps_the_os_error_as_the_source() {
    let error = Error::write_failed(Utf8Path::new("/data/conv/events.json"), os_disk_full());

    let source = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("the os error is retained as the source, unwrapped");
    assert_eq!(source.kind(), io::ErrorKind::StorageFull);
}

#[cfg(unix)]
#[test]
fn io_variant_renders_the_os_error_verbatim() {
    // `Error::Io` is transparent: a bare I/O failure must not be flattened into
    // an opaque "IO error", or the cause is lost wherever only `Display` is
    // rendered.
    let error = Error::from(os_disk_full());

    assert_eq!(error.to_string(), "No space left on device (os error 28)");
}
