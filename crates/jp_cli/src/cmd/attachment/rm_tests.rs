use std::fs;

use camino_tempfile::tempdir;
use jp_config::conversation::attachment::{PartialAttachmentConfig, PartialAttachmentObjectConfig};
use relative_path::RelativePathBuf;

use super::*;

#[cfg(not(windows))]
const NAMES: [&str; 3] = [
    "reports/Issue #42.md",
    "reports/what?.md",
    "reports/100%25.md",
];

// Windows rejects `?` in file names.
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

    // `parse` resolves relative paths against the process working directory, so
    // pass absolute paths under `root`. Construct `AttachmentUrlOrPath::Path`
    // directly because `from_str` parses a Windows drive letter (`C:\...`) as a
    // URL scheme.
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
