use clap::CommandFactory as _;

use super::*;

fn parse(args: &[&str]) -> Command {
    Cli::try_parse_from(args).unwrap().command
}

/// `render`'s flags, as `changelog render` reads `args`.
fn render(args: &[&str]) -> Render {
    let mut command = vec!["changelog", "render"];
    command.extend_from_slice(args);

    let Command::Render { render, .. } = parse(&command) else {
        panic!("expected render");
    };
    render
}

#[test]
fn the_command_line_is_well_formed() {
    Cli::command().debug_assert();
}

#[test]
fn check_reads_changelog_d_unless_given_another_directory() {
    let Command::Check { dir } = parse(&["changelog", "check"]) else {
        panic!("expected check");
    };
    assert_eq!(dir.as_str(), "changelog.d");

    let Command::Check { dir } = parse(&["changelog", "check", "docs/changes"]) else {
        panic!("expected check");
    };
    assert_eq!(dir.as_str(), "docs/changes");
}

#[test]
fn render_takes_the_settings_jp_reads_from_configuration_as_flags() {
    let Command::Render {
        dir,
        forge,
        headings,
        ..
    } = parse(&[
        "changelog",
        "render",
        "--dir",
        "docs/changes",
        "--forge",
        "gitlab",
        "--heading",
        "added=New features",
        "--heading",
        "fixed = Bug fixes",
    ])
    else {
        panic!("expected render");
    };

    assert_eq!(dir.as_str(), "docs/changes");
    assert_eq!(forge, Some(Forge::GitLab));
    assert_eq!(headings, [
        (Type::Added, "New features".to_owned()),
        (Type::Fixed, "Bug fixes".to_owned()),
    ]);
}

#[test]
fn render_reads_the_unreleased_fragments_from_changelog_d_by_default() {
    let Command::Render {
        render,
        dir,
        forge,
        headings,
    } = parse(&["changelog", "render"])
    else {
        panic!("expected render");
    };

    assert_eq!(render.part(), Part::Unreleased);
    assert_eq!(dir.as_str(), "changelog.d");
    assert_eq!(forge, None);
    assert_eq!(headings, []);
}

#[test]
fn a_release_is_named_by_its_version_with_or_without_its_v() {
    assert_eq!(
        render(&["0.3.0"]).part(),
        Part::Release(Version::new(0, 3, 0))
    );
    assert_eq!(
        render(&["v0.3.0"]).part(),
        Part::Release(Version::new(0, 3, 0))
    );

    assert!(Cli::try_parse_from(["changelog", "render", "0.3"]).is_err());
}

#[test]
fn a_heading_names_a_type_and_its_text() {
    assert_eq!(
        heading("security=Security fixes"),
        Ok((Type::Security, "Security fixes".to_owned()))
    );
    assert_eq!(
        heading("New features"),
        Err("`New features` is not `TYPE=HEADING`, such as `added=New features`".to_owned())
    );
    assert_eq!(
        heading("added= "),
        Err("the heading for `added` is empty".to_owned())
    );
    assert!(
        heading("feat=Features")
            .unwrap_err()
            .starts_with("`feat` is not a changelog type")
    );
}

#[test]
fn the_release_notes_link_to_an_upgrade_page_beside_them_by_default() {
    assert_eq!(
        render(&[]).view(Format::Markdown),
        Ok(View::Notes {
            upgrade_guide: "upgrade".to_owned()
        })
    );
    assert_eq!(
        render(&[
            "--upgrade-url",
            "https://jp.computer/releases/0.3.0/upgrade"
        ])
        .view(Format::Markdown),
        Ok(View::Notes {
            upgrade_guide: "https://jp.computer/releases/0.3.0/upgrade".to_owned()
        })
    );
    assert_eq!(
        render(&["--page", "upgrade"]).view(Format::Markdown),
        Ok(View::UpgradeGuide)
    );
}

/// A caller can make JSON the default, and `--format` still decides.
#[test]
fn the_format_is_the_callers_default_unless_named() {
    assert_eq!(render(&[]).view(Format::Json), Ok(View::Json));
    assert_eq!(
        render(&["--format", "json"]).view(Format::Markdown),
        Ok(View::Json)
    );
    assert!(matches!(
        render(&["--format", "markdown"]).view(Format::Json),
        Ok(View::Notes { .. })
    ));
}

#[test]
fn a_flag_that_does_not_apply_to_what_is_printed_is_refused() {
    assert!(
        render(&["--format", "json", "--page", "upgrade"])
            .view(Format::Markdown)
            .unwrap_err()
            .starts_with("`--page` names a Markdown document")
    );
    assert!(
        render(&["--format", "json", "--upgrade-url", "upgrade"])
            .view(Format::Markdown)
            .unwrap_err()
            .starts_with("`--upgrade-url` is where the Markdown release notes link to")
    );
    assert!(
        render(&["--page", "upgrade", "--upgrade-url", "upgrade"])
            .view(Format::Markdown)
            .unwrap_err()
            .starts_with("`--upgrade-url` is where the release notes link to the upgrade guide")
    );
}
