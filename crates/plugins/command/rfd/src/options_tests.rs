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
            dir: Utf8PathBuf::from("docs/rfd"),
            categories: vec![
                "design".to_owned(),
                "decision".to_owned(),
                "guide".to_owned(),
                "process".to_owned(),
            ],
        })
    );
}

#[test]
fn configured_options_replace_the_defaults() {
    let options = read(json!({ "dir": "design", "categories": ["adr", "how-to"] })).unwrap();

    assert_eq!(options, Options {
        dir: Utf8PathBuf::from("design"),
        categories: vec!["adr".to_owned(), "how-to".to_owned()],
    });
}

/// A category names a template file and a filter value, so it is read the one
/// way both can use.
#[test]
fn categories_are_lowercased_and_named_once() {
    let options = read(json!({ "categories": ["Design", " design ", "ADR"] })).unwrap();

    assert_eq!(options.categories, ["design", "adr"]);
}

#[test]
fn a_category_that_cannot_name_a_file_is_refused() {
    for category in ["", "two words", "a/b", "-dash"] {
        assert!(
            read(json!({ "categories": [category] })).is_err(),
            "{category:?} was accepted"
        );
    }

    assert_eq!(
        read(json!({ "categories": ["a/b"] })),
        Err(
            "`plugins.command.rfd.options.categories` holds `a/b`; a category is letters, digits, \
             and dashes, like `design`."
                .to_owned()
        )
    );
}

#[test]
fn an_empty_category_list_is_refused() {
    assert_eq!(
        read(json!({ "categories": [] })),
        Err(
            "`plugins.command.rfd.options.categories` is empty; name at least one category."
                .to_owned()
        )
    );
}

#[test]
fn an_option_of_the_wrong_type_is_named() {
    let error = read(json!({ "categories": "design" })).unwrap_err();

    assert!(
        error.starts_with("`plugins.command.rfd.options` is invalid: invalid type: string"),
        "{error}"
    );
}

#[test]
fn an_unknown_option_is_ignored() {
    assert_eq!(read(json!({ "milestones": [] })), Ok(Options::default()));
}

#[test]
fn an_empty_directory_is_refused() {
    assert_eq!(
        read(json!({ "dir": " " })),
        Err("`plugins.command.rfd.options.dir` is empty.".to_owned())
    );
}

/// The ticket directory is `jp ticket`'s setting, read where that plugin reads
/// it.
#[test]
fn the_ticket_directory_comes_from_the_ticket_plugins_options() {
    assert_eq!(ticket_dir(&json!({})), Ok(Utf8PathBuf::from("docs/ticket")));
    assert_eq!(
        ticket_dir(&json!({ "plugins": { "command": { "ticket": { "run": "allow" } } } })),
        Ok(Utf8PathBuf::from("docs/ticket"))
    );
    assert_eq!(
        ticket_dir(&json!({
            "plugins": { "command": { "ticket": { "options": { "dir": "work/items" } } } }
        })),
        Ok(Utf8PathBuf::from("work/items"))
    );
    assert_eq!(
        ticket_dir(&json!({
            "plugins": { "command": { "ticket": { "options": { "dir": 7 } } } }
        })),
        Err("`plugins.command.ticket.options.dir` is not a directory path.".to_owned())
    );
}

/// The label vocabulary is `jp ticket`'s setting too: a tracking ticket carries
/// a label the ticket board has to define.
#[test]
fn the_ticket_vocabulary_comes_from_the_ticket_plugins_options() {
    assert!(ticket_vocabulary(&json!({})).unwrap().is_empty());

    let vocabulary = ticket_vocabulary(&json!({
        "plugins": { "command": { "ticket": { "options": {
            "labels": { "type": { "description": "The type", "values": ["tracking"] } }
        } } } }
    }))
    .unwrap();
    assert_eq!(vocabulary.tokens(), ["type=tracking"]);

    let error = ticket_vocabulary(&json!({
        "plugins": { "command": { "ticket": { "options": { "labels": { "type": "x" } } } } }
    }))
    .unwrap_err();
    assert!(
        error.starts_with(
            "`plugins.command.ticket.options.labels`: The label vocabulary is not a map"
        ),
        "{error}"
    );
}
