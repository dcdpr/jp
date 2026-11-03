use std::fs;

use camino_tempfile::tempdir;
use pretty_assertions::assert_eq;

use super::{Config, FILE_NAME};
use crate::{Error, format::FormatOptions};

#[test]
fn every_key_parses() {
    let config = Config::parse(
        "max_width = 72\nformat_markdown = true\nreference_links = false\nprune_reference_links = \
         true\n",
    )
    .unwrap();

    assert_eq!(config, Config {
        max_width: Some(72),
        format_markdown: Some(true),
        reference_links: Some(false),
        prune_reference_links: Some(true),
    });
}

#[test]
fn an_empty_file_sets_nothing() {
    assert_eq!(Config::parse("").unwrap(), Config::default());
}

#[test]
fn an_unknown_key_is_refused() {
    let error = Config::parse("format_markdwon = true\n").unwrap_err();

    assert!(
        error.to_string().contains("format_markdwon"),
        "the error doesn't name the key: {error}"
    );
}

#[test]
fn unset_settings_take_the_flag_defaults() {
    assert_eq!(Config::default().format_options(), FormatOptions {
        max_width: 80,
        canonical: false,
        reference_links: false,
        prune_reference_links: false,
    });
}

#[test]
fn set_settings_reach_the_format_options() {
    let config = Config {
        max_width: Some(0),
        format_markdown: Some(true),
        reference_links: Some(true),
        prune_reference_links: Some(true),
    };

    assert_eq!(config.format_options(), FormatOptions {
        max_width: 0,
        canonical: true,
        reference_links: true,
        prune_reference_links: true,
    });
}

#[test]
fn an_override_wins_only_where_it_sets_something() {
    let file = Config {
        max_width: Some(100),
        format_markdown: Some(true),
        reference_links: Some(true),
        prune_reference_links: None,
    };
    let flags = Config {
        max_width: None,
        format_markdown: Some(false),
        reference_links: None,
        prune_reference_links: Some(true),
    };

    assert_eq!(file.overridden_by(flags), Config {
        max_width: Some(100),
        format_markdown: Some(false),
        reference_links: Some(true),
        prune_reference_links: Some(true),
    });
}

#[test]
fn read_in_a_directory_without_a_file_is_the_default() {
    let dir = tempdir().unwrap();

    assert_eq!(
        Config::read_in(dir.path().as_std_path()).unwrap(),
        Config::default()
    );
}

#[test]
fn read_in_reads_the_file_in_the_directory() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(FILE_NAME), "max_width = 60\n").unwrap();

    assert_eq!(Config::read_in(dir.path().as_std_path()).unwrap(), Config {
        max_width: Some(60),
        ..Config::default()
    });
}

#[test]
fn read_in_names_the_file_that_does_not_parse() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(FILE_NAME), "max_width = \"wide\"\n").unwrap();

    let error = Config::read_in(dir.path().as_std_path()).unwrap_err();

    let Error::Config { path, .. } = &error else {
        panic!("expected a config error, got {error:?}");
    };
    assert_eq!(path, &dir.path().join(FILE_NAME).into_std_path_buf());
}

#[test]
fn find_takes_the_nearest_file_above_the_start() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    fs::write(dir.path().join(FILE_NAME), "").unwrap();
    fs::write(dir.path().join("a").join(FILE_NAME), "").unwrap();

    assert_eq!(
        Config::find(nested.as_std_path()),
        Some(dir.path().join("a").join(FILE_NAME).into_std_path_buf())
    );
}
