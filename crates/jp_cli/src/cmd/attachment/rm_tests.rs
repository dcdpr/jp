use std::fs;

use camino_tempfile::tempdir;
use jp_config::conversation::attachment::{PartialAttachmentConfig, PartialAttachmentObjectConfig};
use relative_path::RelativePathBuf;

use super::*;

/// File names holding characters a URL reads as delimiters.
#[cfg(not(windows))]
const NAMES: [&str; 3] = [
    "reports/Issue #42.md",
    "reports/what?.md",
    "reports/100%25.md",
];

/// Windows refuses `?` in a file name, so that case runs on other platforms
/// only.
#[cfg(windows)]
const NAMES: [&str; 2] = ["reports/Issue #42.md", "reports/100%25.md"];

fn file_table(path: &str) -> PartialAttachmentConfig {
    PartialAttachmentConfig::Object(PartialAttachmentObjectConfig {
        kind: Some("file".to_owned()),
        path: Some(path.to_owned()),
        params: None,
    })
}

/// A path given on the command line names the same attachment as a `file` table
/// with that `path`, including characters a URL reads as delimiters.
#[test]
fn rm_of_a_bare_path_removes_the_file_table_for_it() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("reports")).unwrap();
    for name in NAMES {
        fs::write(root.path().join(name), "").unwrap();
    }

    let workspace = Workspace::in_memory(root.path().to_path_buf());
    let mut partial = PartialAppConfig::empty();
    for name in NAMES {
        partial.conversation.attachments.push(file_table(name));
    }

    // `parse` resolves a relative path against the process directory, so the
    // paths are given absolute, as a shell completing them under the root would.
    // They are built as paths directly: `from_str` would read a Windows drive
    // letter as a URL scheme.
    let rm = Rm {
        attachments: NAMES
            .iter()
            .map(|name| {
                AttachmentUrlOrPath::Path(RelativePathBuf::from(
                    root.path().join(name).into_string(),
                ))
            })
            .collect(),
    };

    let partial = rm
        .apply_cli_config(Some(&workspace), partial, None)
        .unwrap();

    assert_eq!(
        partial.conversation.attachments.to_vec(),
        Vec::<PartialAttachmentConfig>::new()
    );
}
