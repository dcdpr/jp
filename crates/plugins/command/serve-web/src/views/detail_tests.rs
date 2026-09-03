use pretty_assertions::assert_eq;

use super::*;

/// The dialog claims focus itself when it opens.
///
/// Left to the browser, focus goes to the dialog's first field.
/// Once the chooser has been fetched and a row written into it, that is the
/// row's own field, so on a touch device opening the dialog raises the keyboard
/// over it and iOS zooms the page to meet the field.
#[test]
fn the_configuration_dialog_takes_focus_itself() {
    let markup = config_modal().into_string();
    let opening = markup
        .split_once('>')
        .map(|(tag, _)| tag)
        .unwrap_or_default();

    assert_eq!(
        opening,
        r#"<dialog id="config-modal" class="config-modal" tabindex="-1" autofocus"#
    );
}
