use super::{demangle, is_mangled};

#[test]
fn recognises_swift_manglings() {
    assert!(is_mangled("$s2JP17ConversationEventV4bodyQrvg"));
    assert!(is_mangled("_$s2JP17ConversationEventV4bodyQrvg"));
    assert!(is_mangled("$S3foo3barC"));
    assert!(is_mangled("_T0Si"));
}

#[test]
fn ignores_other_manglings() {
    assert!(!is_mangled("_ZN3std2io4Read11read_to_endE"));
    assert!(!is_mangled("_RNvCskwGfYPst2Cb_3foo3bar"));
    assert!(!is_mangled("-[NSView drawRect:]"));
    assert!(!is_mangled("main"));
    assert!(!is_mangled(""));
}

/// The demangler is loaded out of the installed Xcode, so this asserts against
/// a real toolchain rather than a vendored copy.
/// A missing dylib fails the test instead of skipping it: on a machine that can
/// build the macOS app, the dylib is always there, and a silent skip would let
/// the whole feature rot.
#[cfg(target_os = "macos")]
#[test]
fn demangles_through_the_toolchain() {
    assert_eq!(
        demangle("$s4main5helloSSyYaKF").as_deref(),
        Some("main.hello() async throws -> Swift.String")
    );

    // The shape a Time Profiler row reports for one of our own accessors,
    // with the leading underscore the Mach-O symbol table carries.
    assert_eq!(
        demangle("_$s2JP17ConversationEventV4bodyQrvg").as_deref(),
        Some("JP.ConversationEvent.body.getter : some")
    );
}

#[test]
fn passes_through_non_swift_symbols() {
    assert_eq!(demangle("_ZN3std2io4Read11read_to_endE"), None);
    assert_eq!(demangle("main"), None);
}
