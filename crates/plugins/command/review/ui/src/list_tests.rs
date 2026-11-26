use pretty_assertions::assert_eq;

use super::*;

#[test]
fn the_files_are_read_one_per_line_or_comma() {
    assert_eq!(paths(" docs/a.md\n\ndocs/b.md, docs/c.md ,"), [
        "docs/a.md",
        "docs/b.md",
        "docs/c.md"
    ]);
    assert_eq!(paths("  \n"), Vec::<String>::new());
}
