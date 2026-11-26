use pretty_assertions::assert_eq;

use super::*;

/// The file the links are written in.
const FROM: &str = "docs/rfd/140-review.md";

fn file(path: &str, heading: Option<&str>) -> Target {
    Target::File {
        path: path.to_owned(),
        heading: heading.map(ToOwned::to_owned),
    }
}

#[test]
fn a_link_beside_the_file_leads_to_its_neighbour() {
    assert_eq!(
        resolve("139-leptos.md", FROM),
        file("docs/rfd/139-leptos.md", None)
    );
    assert_eq!(
        resolve("./139-leptos.md", FROM),
        file("docs/rfd/139-leptos.md", None)
    );
    assert_eq!(
        resolve("drafts/D01-kb.md", FROM),
        file("docs/rfd/drafts/D01-kb.md", None)
    );
}

#[test]
fn a_link_climbs_out_of_the_files_directory() {
    assert_eq!(
        resolve("../wiki/index.md", FROM),
        file("docs/wiki/index.md", None)
    );
    assert_eq!(resolve("../../justfile", FROM), file("justfile", None));
}

#[test]
fn a_link_from_a_file_at_the_root_is_read_from_the_root() {
    assert_eq!(
        resolve("docs/usage.md", "README.md"),
        file("docs/usage.md", None)
    );
}

#[test]
fn a_link_names_a_heading_of_the_file_it_leads_to() {
    assert_eq!(
        resolve("139-leptos.md#styling", FROM),
        file("docs/rfd/139-leptos.md", Some("styling"))
    );
    assert_eq!(
        resolve("139-leptos.md#", FROM),
        file("docs/rfd/139-leptos.md", None),
        "an empty fragment names none"
    );
}

#[test]
fn a_fragment_alone_names_a_heading_of_the_file_itself() {
    assert_eq!(
        resolve("#commenting", FROM),
        Target::Heading("commenting".to_owned())
    );
    assert_eq!(
        resolve("#caf%C3%A9", FROM),
        Target::Heading("café".to_owned())
    );
}

#[test]
fn escapes_in_a_path_are_decoded_and_a_plus_is_kept() {
    assert_eq!(
        resolve("my%20notes.md", FROM),
        file("docs/rfd/my notes.md", None)
    );
    assert_eq!(resolve("c++.md", FROM), file("docs/rfd/c++.md", None));
}

#[test]
fn a_query_is_dropped() {
    assert_eq!(
        resolve("139-leptos.md?plain=1#styling", FROM),
        file("docs/rfd/139-leptos.md", Some("styling"))
    );
}

#[test]
fn an_address_with_a_scheme_leads_elsewhere() {
    assert_eq!(resolve("https://plannotator.ai/", FROM), Target::Elsewhere);
    assert_eq!(resolve("mailto:jane@example.com", FROM), Target::Elsewhere);
    assert_eq!(resolve("//example.com/a.md", FROM), Target::Elsewhere);
}

/// A colon after a `/` is part of a file's name, and opens no scheme.
#[test]
fn a_colon_in_a_later_part_of_a_path_is_no_scheme() {
    assert_eq!(
        resolve("notes/10:30.md", FROM),
        file("docs/rfd/notes/10:30.md", None)
    );
}

#[test]
fn a_link_out_of_the_workspace_leads_nowhere() {
    assert_eq!(resolve("../../../etc/hosts", FROM), Target::Nowhere);
    assert_eq!(resolve("/rfd/139-leptos", FROM), Target::Nowhere);
    assert_eq!(resolve("", FROM), Target::Nowhere);
    assert_eq!(resolve("..", "README.md"), Target::Nowhere);
}

/// The directory itself, which is no file.
#[test]
fn a_link_to_the_root_directory_leads_nowhere() {
    assert_eq!(resolve("./", "README.md"), Target::Nowhere);
}
