//! The `changelog` binary, and the flags of its `render` that `jp release
//! render` shares.
//!
//! `changelog check [DIR]` holds a fragments directory, `changelog.d` unless
//! `DIR` names another, to its layout and every fragment and highlight in it to
//! its format, and exits 1 when anything breaks a rule.
//!
//! `changelog render [VERSION]` prints a release's notes or upgrade guide, or
//! the fragments no release has taken yet, as Markdown or as the JSON other
//! tools render from ([`Render`]).
//! The forge and the headings arrive as flags rather than from configuration.
//!
//! Neither reads configuration, so CI can run them without `jp`.

use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use semver::Version;

use crate::{
    directory::{self, Part},
    forge::Forge,
    fragment::Type,
    markdown::{self, UPGRADE_GUIDE},
    release::{self, Headings, Release},
};

#[derive(Debug, Parser)]
#[command(
    name = "changelog",
    about = "Check the changelog fragments and highlights a project records for its users, and \
             render its release notes from them.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check every file in a fragments directory, released ones included.
    ///
    /// Prints each problem, then how many fragments and highlights were
    /// checked.
    /// Exits 1 when anything breaks a rule, and 2 when a file or directory
    /// can't be read.
    Check {
        /// The fragments directory.
        #[arg(default_value = "changelog.d")]
        dir: Utf8PathBuf,
    },

    /// Print a release's notes or upgrade guide, or the fragments no release
    /// has taken yet.
    ///
    /// Each fragment and highlight is credited to the change requests that
    /// wrote it, read from the history of the repository holding the fragments
    /// directory, so a shallow clone is refused.
    /// Exits 1 when the release can't be read or rendered.
    Render {
        #[command(flatten)]
        render: Render,

        /// The fragments directory.
        #[arg(long, default_value = "changelog.d")]
        dir: Utf8PathBuf,

        /// Where change requests are reviewed: `github`, `gitlab`, or `none`.
        ///
        /// Without it, the forge is read from the address of git's `origin`
        /// remote: `github` for github.com, `gitlab` for gitlab.com, and `none`
        /// for any other host.
        #[arg(long)]
        forge: Option<Forge>,

        /// The heading of a type's section of the release notes, such as
        /// `added=New features`.
        ///
        /// Repeat it for each type to rename.
        /// A type without one is headed by its id capitalized, such as `Added`.
        #[arg(long = "heading", value_name = "TYPE=HEADING", value_parser = heading)]
        headings: Vec<(Type, String)>,
    },
}

/// What `render` prints, as `changelog render` and `jp release render` take it.
// No group of its own: a subcommand flattening it under the same name would
// define the group twice.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[group(skip)]
pub struct Render {
    /// The release, such as `0.3.0`; without one, the fragments no release has
    /// taken yet.
    #[arg(value_parser = version)]
    pub version: Option<Version>,

    /// `markdown` for a document to read, or `json` for the release as other
    /// tools render it.
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// The Markdown document: `notes` for the release notes, the default, or
    /// `upgrade` for the upgrade guide.
    #[arg(long, value_enum)]
    pub page: Option<Page>,

    /// Where the release notes link to the upgrade guide, such as
    /// `https://jp.computer/releases/0.3.0/upgrade`.
    ///
    /// A breaking entry links to its section there, after a `#`.
    /// Defaults to `upgrade`, a page beside the release notes.
    #[arg(long, value_name = "URL")]
    pub upgrade_url: Option<String>,
}

/// The format `render` prints in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Markdown,
    Json,
}

/// The Markdown document `render` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Page {
    Notes,
    Upgrade,
}

/// What to print of a release, its flags checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The release, as the JSON other tools render from.
    Json,

    /// The release notes, linking to the upgrade guide at this address.
    Notes { upgrade_guide: String },

    /// The upgrade guide.
    UpgradeGuide,
}

impl Render {
    /// The part of the fragments directory to render.
    #[must_use]
    pub fn part(&self) -> Part {
        self.version.clone().map_or(Part::Unreleased, Part::Release)
    }

    /// What to print, in `default` unless `--format` names a format.
    ///
    /// # Errors
    ///
    /// Refuses a flag that doesn't apply to what it would print.
    pub fn view(&self, default: Format) -> Result<View, String> {
        match (self.format.unwrap_or(default), self.page) {
            (Format::Json, Some(_)) => Err("`--page` names a Markdown document; JSON holds the \
                                            whole release, so drop it or add `--format markdown`."
                .to_owned()),
            (Format::Json, None) if self.upgrade_url.is_some() => Err("`--upgrade-url` is where \
                                                                       the Markdown release \
                                                                       notes link to; drop it or \
                                                                       add `--format markdown`."
                .to_owned()),
            (Format::Json, None) => Ok(View::Json),
            (Format::Markdown, Some(Page::Upgrade)) if self.upgrade_url.is_some() => Err(
                "`--upgrade-url` is where the release notes link to the upgrade guide; it has no \
                 use for the upgrade guide itself."
                    .to_owned(),
            ),
            (Format::Markdown, Some(Page::Upgrade)) => Ok(View::UpgradeGuide),
            (Format::Markdown, Some(Page::Notes) | None) => Ok(View::Notes {
                upgrade_guide: self
                    .upgrade_url
                    .clone()
                    .unwrap_or_else(|| UPGRADE_GUIDE.to_owned()),
            }),
        }
    }
}

impl View {
    /// `release` as this view prints it, each type's entries headed by
    /// `headings`, and ending in a line break unless there is nothing to print:
    /// a release without upgrade sections has no upgrade guide.
    ///
    /// # Errors
    ///
    /// Returns why the JSON couldn't be written.
    pub fn render(&self, release: &Release, headings: &Headings) -> Result<String, String> {
        Ok(match self {
            Self::Json => {
                let json = serde_json::to_string_pretty(release)
                    .map_err(|error| format!("failed to write the release as JSON: {error}"))?;
                json + "\n"
            }
            Self::Notes { upgrade_guide } => markdown::notes(release, headings, upgrade_guide),
            Self::UpgradeGuide => markdown::upgrade_guide(release),
        })
    }
}

/// Read a release's version, such as `0.3.0`, with or without the `v` its tag
/// has.
///
/// # Errors
///
/// Says why `text` isn't a semantic version.
pub fn version(text: &str) -> Result<Version, String> {
    let bare = text.strip_prefix('v').unwrap_or(text);

    Version::parse(bare)
        .map_err(|error| format!("`{text}` is not a release's version, such as `0.3.0`: {error}"))
}

/// Read `--heading`'s `TYPE=HEADING`.
fn heading(text: &str) -> Result<(Type, String), String> {
    let (type_, heading) = text
        .split_once('=')
        .ok_or_else(|| format!("`{text}` is not `TYPE=HEADING`, such as `added=New features`"))?;
    let type_ = type_
        .trim()
        .parse::<Type>()
        .map_err(|error| error.to_string())?;
    let heading = heading.trim();
    if heading.is_empty() {
        return Err(format!("the heading for `{type_}` is empty"));
    }

    Ok((type_, heading.to_owned()))
}

/// Run the binary with the process's arguments.
#[must_use]
// The binary's output: the report or the release on stdout, a failure on
// stderr.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check { dir } => match directory::check(&dir) {
            Ok(report) => {
                println!("{report}");
                ExitCode::from(u8::from(!report.problems.is_empty()))
            }
            Err(error) => {
                eprintln!("changelog: {error}");
                ExitCode::from(2)
            }
        },
        Command::Render {
            render,
            dir,
            forge,
            headings,
        } => {
            let headings = Headings::from_iter(headings);
            let printed = render.view(Format::Markdown).and_then(|view| {
                let release = release::read(&dir, &render.part(), forge, &headings)
                    .map_err(|error| error.to_string())?;
                view.render(&release, &headings)
            });

            match printed {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("changelog: {error}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
