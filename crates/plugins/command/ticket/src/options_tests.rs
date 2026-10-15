use serde_json::json;

use super::*;

fn read(options: Value) -> Result<Options, String> {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options)
}

#[test]
fn every_option_has_a_default() {
    assert_eq!(
        read(json!({})),
        Ok(Options {
            dir: Utf8PathBuf::from("docs/ticket"),
            assistant: "assistant".to_owned(),
            board: BoardOptions { done_limit: 8 },
        })
    );
}

#[test]
fn configured_options_replace_the_defaults() {
    let options = read(json!({
        "dir": "tickets",
        "assistant": "jp",
        "board": { "done_limit": 3 }
    }))
    .unwrap();

    assert_eq!(options, Options {
        dir: Utf8PathBuf::from("tickets"),
        assistant: "jp".to_owned(),
        board: BoardOptions { done_limit: 3 },
    });
}

/// Options merge key by key across config layers, so any one of them can arrive
/// on its own.
#[test]
fn an_option_left_out_keeps_its_default() {
    let options = read(json!({ "board": {} })).unwrap();

    assert_eq!(options.board.done_limit, 8);
    assert_eq!(options.dir, "docs/ticket");
}

#[test]
fn an_option_of_the_wrong_type_is_named() {
    let error = read(json!({ "board": { "done_limit": "eight" } })).unwrap_err();

    assert!(
        error.starts_with("`plugins.command.ticket.options` is invalid: invalid type: string"),
        "{error}"
    );
}

/// The vocabulary joins these options later; a configuration that already
/// carries it, or anything else this build doesn't read, must still load.
#[test]
fn an_unknown_option_is_ignored() {
    let options = read(json!({ "labels": { "type": { "values": ["bug"] } } })).unwrap();

    assert_eq!(options, Options::default());
}

/// The assistant's name is written on a metadata line, so a blank one or one
/// that spans lines would leave tickets with no author or a broken header.
#[test]
fn the_assistant_has_to_be_a_name_on_one_line() {
    for assistant in ["  ", "jp\nand more"] {
        assert_eq!(
            read(json!({ "assistant": assistant })),
            Err(
                "`plugins.command.ticket.options.assistant` must be a name on one line.".to_owned()
            )
        );
    }

    assert_eq!(
        read(json!({ "assistant": " jp " })).unwrap().assistant,
        "jp"
    );
}

#[test]
fn an_empty_directory_is_refused() {
    assert_eq!(
        read(json!({ "dir": "" })),
        Err("`plugins.command.ticket.options.dir` is empty.".to_owned())
    );
}
