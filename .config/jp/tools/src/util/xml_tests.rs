use super::*;

#[test]
fn renders_items_as_bullets_inside_the_root_tags() {
    let items = ["a.txt", "src/lib.rs"];

    assert_eq!(to_list_with_root(&items, "results"), indoc::indoc! {"
        <results>
        - a.txt
        - src/lib.rs
        </results>"});
}

#[test]
fn renders_no_items_as_an_empty_block() {
    let items: [&str; 0] = [];

    assert_eq!(
        to_list_with_root(&items, "results"),
        "<results>\n</results>"
    );
}
