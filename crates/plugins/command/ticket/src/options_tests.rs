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
            labels: None,
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
        labels: None,
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

/// A configuration written for a newer `jp-ticket` must still load.
#[test]
fn an_unknown_option_is_ignored() {
    let options = read(json!({ "board": { "columns": 4 }, "future": true })).unwrap();

    assert_eq!(options, Options::default());
}

#[test]
fn a_board_without_labels_defines_none() {
    assert!(read(json!({})).unwrap().vocabulary().unwrap().is_empty());
}

#[test]
fn the_vocabulary_is_read_from_labels() {
    let options = read(json!({
        "labels": {
            "type": { "description": "The type of ticket", "values": ["bug", "tracking"] }
        }
    }))
    .unwrap();

    let vocabulary = options.vocabulary().unwrap();

    assert_eq!(vocabulary.tokens(), ["type=bug", "type=tracking"]);
}

/// Read when a command needs it, so a broken vocabulary loads with the rest of
/// the options and fails only the commands that label.
#[test]
fn a_broken_vocabulary_is_named_by_where_it_is_configured() {
    let options = read(json!({ "labels": { "type": "The type of ticket" } })).unwrap();

    let error = options.vocabulary().unwrap_err();

    assert!(
        error.starts_with(
            "`plugins.command.ticket.options.labels`: The label vocabulary is not a map"
        ),
        "{error}"
    );
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
