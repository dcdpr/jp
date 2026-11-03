use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use comfort::format::FormatOptions;
use serde_json::json;

use super::{Formatting, format_on_write};

/// Sentence per line, so a formatted file is told from an unformatted one.
const STYLE: FormatOptions = FormatOptions {
    max_width: 80,
    canonical: true,
    reference_links: true,
    prune_reference_links: true,
};

const UNFORMATTED: &str = "# Title\n\nFirst sentence. Second sentence.\n";
const FORMATTED: &str = "# Title\n\nFirst sentence.\nSecond sentence.\n";

fn tickets() -> Formatting {
    Formatting::new(STYLE, vec![Utf8PathBuf::from("/w/docs/ticket")])
}

#[test]
fn a_markdown_file_in_a_formatted_directory_is_formatted() {
    let path = Utf8Path::new("/w/docs/ticket/0a1b2c3-slug.md");

    assert_eq!(tickets().apply(path, UNFORMATTED), FORMATTED);
}

#[test]
fn a_file_in_a_nested_directory_is_formatted() {
    let path = Utf8Path::new("/w/docs/ticket/drafts/D01-slug.md");

    assert_eq!(tickets().apply(path, UNFORMATTED), FORMATTED);
}

#[test]
fn a_file_outside_every_formatted_directory_is_left_alone() {
    let path = Utf8Path::new("/w/docs/rfd/042-slug.md");

    assert_eq!(tickets().apply(path, UNFORMATTED), UNFORMATTED);
}

#[test]
fn a_directory_sharing_a_prefix_is_not_inside() {
    // `docs/tickets` starts with the string `docs/ticket`, but not with the
    // path.
    let path = Utf8Path::new("/w/docs/tickets/0a1b2c3-slug.md");

    assert_eq!(tickets().apply(path, UNFORMATTED), UNFORMATTED);
}

#[test]
fn a_file_that_is_not_markdown_is_left_alone() {
    let path = Utf8Path::new("/w/docs/ticket/.board.json");

    assert_eq!(tickets().apply(path, UNFORMATTED), UNFORMATTED);
}

#[test]
fn the_default_formats_nothing() {
    let path = Utf8Path::new("/w/docs/ticket/0a1b2c3-slug.md");

    assert_eq!(Formatting::default().apply(path, UNFORMATTED), UNFORMATTED);
}

#[test]
fn write_formats_what_it_writes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("0a1b2c3-slug.md");
    let formatting = Formatting::new(STYLE, vec![dir.path().to_path_buf()]);

    formatting.write(&path, UNFORMATTED).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), FORMATTED);
}

#[test]
fn format_on_write_is_off_when_unset() {
    assert_eq!(format_on_write(&json!({}), "ticket"), Ok(false));
    assert_eq!(
        format_on_write(
            &json!({ "plugins": { "command": { "ticket": { "options": {} } } } }),
            "ticket"
        ),
        Ok(false)
    );
}

#[test]
fn format_on_write_reads_the_plugin_s_own_option() {
    let config = json!({ "plugins": { "command": {
        "ticket": { "options": { "format_on_write": true } },
        "rfd": { "options": { "format_on_write": false } },
    } } });

    assert_eq!(format_on_write(&config, "ticket"), Ok(true));
    assert_eq!(format_on_write(&config, "rfd"), Ok(false));
    assert_eq!(format_on_write(&config, "wiki"), Ok(false));
}

#[test]
fn format_on_write_that_is_not_a_boolean_names_the_setting() {
    let config = json!({ "plugins": { "command": {
        "wiki": { "options": { "format_on_write": "yes" } },
    } } });

    assert_eq!(
        format_on_write(&config, "wiki"),
        Err("`plugins.command.wiki.options.format_on_write` must be `true` or `false`.".to_owned())
    );
}

#[test]
fn configure_formats_only_the_kinds_turned_on() {
    let root = tempdir().unwrap();
    fs::write(root.path().join(".comfort.toml"), "max_width = 60\n").unwrap();
    let tickets = root.path().join("docs/ticket");
    let rfds = root.path().join("docs/rfd");
    let config = json!({ "plugins": { "command": {
        "ticket": { "options": { "format_on_write": true } },
    } } });

    let formatting = Formatting::configure(root.path(), &config, &[
        ("ticket", &tickets),
        ("rfd", &rfds),
    ])
    .unwrap();

    assert_eq!(
        formatting,
        Formatting::new(
            FormatOptions {
                max_width: 60,
                ..FormatOptions::default()
            },
            vec![tickets]
        )
    );
}

#[test]
fn configure_without_a_style_file_uses_comfort_s_defaults() {
    let root = tempdir().unwrap();
    let tickets = root.path().join("docs/ticket");
    let config = json!({ "plugins": { "command": {
        "ticket": { "options": { "format_on_write": true } },
    } } });

    let formatting = Formatting::configure(root.path(), &config, &[("ticket", &tickets)]).unwrap();

    assert_eq!(
        formatting,
        Formatting::new(
            FormatOptions {
                max_width: 80,
                ..FormatOptions::default()
            },
            vec![tickets]
        )
    );
}

#[test]
fn configure_with_formatting_off_does_not_read_the_style_file() {
    // A broken style file only matters to a workspace that formats.
    let root = tempdir().unwrap();
    fs::write(root.path().join(".comfort.toml"), "not toml at all [").unwrap();
    let tickets = root.path().join("docs/ticket");

    let formatting = Formatting::configure(root.path(), &json!({}), &[("ticket", &tickets)]);

    assert_eq!(formatting, Ok(Formatting::default()));
}

#[test]
fn configure_reports_a_style_file_that_does_not_parse() {
    let root = tempdir().unwrap();
    fs::write(root.path().join(".comfort.toml"), "max_widht = 60\n").unwrap();
    let tickets = root.path().join("docs/ticket");
    let config = json!({ "plugins": { "command": {
        "ticket": { "options": { "format_on_write": true } },
    } } });

    let error = Formatting::configure(root.path(), &config, &[("ticket", &tickets)]).unwrap_err();

    assert!(
        error.contains(".comfort.toml") && error.contains("max_widht"),
        "the error names neither the file nor the key: {error}"
    );
}
