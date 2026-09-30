use camino_tempfile::tempdir;

use super::*;

/// A newer `jp` sharing the data directory reads the same cache, so an entry
/// this version does not understand is kept in it.
#[test]
fn the_cache_holds_the_registry_as_served() {
    let tmp = tempdir().unwrap();
    let cache = tmp.path().join("registry.json");
    let body = r#"{"version":1,"plugins":{"path":{"id":"path","description":"Paths"},"lint":{"id":"lint","type":"wasm","description":"Lints"}}}"#;

    let registry = parse_and_cache(body, Some(&cache)).unwrap();

    assert_eq!(registry.plugins.keys().collect::<Vec<_>>(), ["path"]);
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), body);
}

/// A body that does not parse is not cached over a good copy.
#[test]
fn an_invalid_registry_leaves_the_cache_alone() {
    let tmp = tempdir().unwrap();
    let cache = tmp.path().join("registry.json");
    std::fs::write(&cache, "the old copy").unwrap();

    assert!(parse_and_cache("{ not json", Some(&cache)).is_err());
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), "the old copy");
}

#[test]
fn sha256_hex_known_value() {
    // SHA-256 of the empty string.
    let hash = sha256_hex(b"");
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn sha256_hex_hello() {
    let hash = sha256_hex(b"hello");
    assert_eq!(
        hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn current_target_has_arch_and_os() {
    let target = current_target();
    assert!(
        target.contains('-'),
        "target should contain a dash: {target}"
    );
    // On any test platform, the arch should be non-empty.
    let arch = target.split('-').next().unwrap();
    assert!(!arch.is_empty());
}

#[test]
fn plugin_binary_name_unix() {
    if !cfg!(windows) {
        assert_eq!(plugin_binary_name("serve"), "jp-serve");
        assert_eq!(plugin_binary_name("my-tool"), "jp-my-tool");
    }
}
