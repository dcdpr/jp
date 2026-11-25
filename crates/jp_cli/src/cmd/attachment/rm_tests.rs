use std::{fs, str::FromStr as _};

use camino_tempfile::tempdir;
use jp_config::conversation::attachment::{PartialAttachmentConfig, PartialAttachmentObjectConfig};

use super::*;

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
    let names = [
        "reports/Issue #42.md",
        "reports/what?.md",
        "reports/100%25.md",
    ];
    fs::create_dir(root.path().join("reports")).unwrap();
    for name in names {
        fs::write(root.path().join(name), "").unwrap();
    }

    let workspace = Workspace::in_memory(root.path().to_path_buf());
    let mut partial = PartialAppConfig::empty();
    for name in names {
        partial.conversation.attachments.push(file_table(name));
    }

    // `parse` resolves a relative path against the process directory, so the
    // paths are given absolute, as a shell completing them under the root would.
    let rm = Rm {
        attachments: names
            .iter()
            .map(|name| AttachmentUrlOrPath::from_str(root.path().join(name).as_str()).unwrap())
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
