use super::*;

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

#[test]
fn strip_plugin_prefix_basic() {
    assert_eq!(strip_plugin_prefix("jp-serve"), Some("serve"));
    assert_eq!(
        strip_plugin_prefix("jp-conversation-export"),
        Some("conversation-export")
    );
    assert_eq!(strip_plugin_prefix("not-a-plugin"), None);
    assert_eq!(strip_plugin_prefix("jp"), None);
}
