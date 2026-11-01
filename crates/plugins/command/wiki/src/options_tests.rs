use serde_json::json;

use super::*;

fn read(value: Value) -> Result<Options, String> {
    let Value::Object(map) = value else {
        panic!("options are a table");
    };

    Options::read(&map)
}

#[test]
fn no_options_reads_as_the_defaults() {
    assert_eq!(read(json!({})), Ok(Options::default()));
}

#[test]
fn a_setting_has_to_read_as_one() {
    assert!(read(json!({ "dir": " " })).is_err());
    assert!(read(json!({ "relevant": { "key": "not a key" } })).is_err());
}

#[test]
fn the_assistant_is_a_name_on_one_line() {
    assert_eq!(
        read(json!({ "assistant": " jp " })).unwrap().assistant,
        "jp"
    );
    assert_eq!(
        read(json!({ "assistant": "two\nlines" })),
        Err("`plugins.command.wiki.options.assistant` must be a name on one line.".to_owned())
    );
    assert!(read(json!({ "assistant": "  " })).is_err());
}

#[test]
fn the_document_directories_come_from_their_plugins() {
    let config = json!({
        "plugins": {
            "command": {
                "rfd": { "options": { "dir": "design" } },
                "ticket": { "options": { "dir": "/abs/tickets" } },
            }
        }
    });

    let paths = Paths::new(Utf8Path::new("/w"), &config, &Options::default()).unwrap();

    assert_eq!(paths.wiki, "/w/docs/wiki");
    assert_eq!(paths.rfds, "/w/design");
    assert_eq!(paths.tickets, "/abs/tickets");
    assert_eq!(paths.docs(), "/w/docs");
}

#[test]
fn a_wiki_at_the_root_is_its_own_documentation_tree() {
    let options = Options {
        dir: Utf8PathBuf::from("wiki"),
        ..Options::default()
    };

    let paths = Paths::new(Utf8Path::new("/w"), &json!({}), &options).unwrap();

    assert_eq!(paths.docs(), "/w/wiki");
    assert_eq!(paths.show(Utf8Path::new("/w/wiki/a.md")), "wiki/a.md");
}

#[test]
fn the_configured_repository_wins() {
    let config = json!({
        "plugins": { "command": { "vite": { "options": {
            "repository": "https://github.com/dcdpr/jp/"
        } } } }
    });

    assert_eq!(
        repository(Utf8Path::new("/nonexistent"), &config),
        Ok(Some("https://github.com/dcdpr/jp".to_owned()))
    );

    let config = json!({
        "plugins": { "command": { "vite": { "options": { "repository": "git@github.com:x/y" } } } }
    });
    assert!(repository(Utf8Path::new("/nonexistent"), &config).is_err());
}
