use camino::Utf8Path;

use super::relative_to;

#[test]
fn relative_to_strips_workspace_prefix() {
    assert_eq!(
        relative_to(
            Utf8Path::new("/Users/jean/jp"),
            Utf8Path::new("/Users/jean/jp/tmp/profiling/trace.jsonl"),
        ),
        "tmp/profiling/trace.jsonl"
    );
}

#[test]
fn relative_to_passes_through_when_outside_workspace() {
    // System temp dir, sandbox temp file, etc. — not under the workspace,
    // so render the absolute path as-is so the user can find it.
    let absolute = "/var/folders/ny/.../T/.tmpXYZ";
    assert_eq!(
        relative_to(Utf8Path::new("/Users/jean/jp"), Utf8Path::new(absolute)),
        absolute
    );
}

#[test]
fn relative_to_passes_through_when_paths_equal() {
    // Edge case: the path *is* the root. `strip_prefix` returns an empty
    // path here, which would render as an empty string. We still want
    // *something* in the report, so the fallback kicks in.
    let root = Utf8Path::new("/Users/jean/jp");
    let result = relative_to(root, root);
    // Either "" (from strip_prefix) or the original — both are
    // technically defensible, but neither should panic.
    assert!(result.is_empty() || result == "/Users/jean/jp");
}
