//! CLI for the standalone `changelog` binary and the [`Render`] flags shared
//! with `jp release render`.
//!
//! - `changelog check [DIR]` validates the layout and file formats of a
//!   fragments directory (`changelog.d` by default) and exits 1 if any problems
//!   are found.
//! - `changelog render [VERSION]` renders a release's notes or upgrade guide
//!   (or all unreleased changes when `VERSION` is omitted) as Markdown or JSON.
//!
//! Neither subcommand reads JP configuration, so CI can run `changelog` without
//! `jp`.

use std::{env, ffi::OsString, process::ExitCode};

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use doc::repository::is_web_address;
use semver::Version;

use crate::{
    directory::{self, Part},
    forge::Forge,
    fragment::Type,
    markdown::{self, UPGRADE_GUIDE},
    release::{self, Headings, Release, ReleaseError},
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
        /// Without it, the forge is read from the address credits link from,
        /// `--repository` or git's `origin` remote: `github` for github.com,
        /// `gitlab` for gitlab.com, and `none` for any other host.
        #[arg(long)]
        forge: Option<Forge>,

        /// The project's web address, which credits link from, such as
        /// `https://github.com/dcdpr/jp`.
        ///
        /// Without it, the address of git's `origin` remote.
        #[arg(long, value_name = "URL", value_parser = repository)]
        repository: Option<String>,

        /// The heading of a type's section of the release notes, such as
        /// `added=New features`.
        ///
        /// Repeat it for each type to rename.
        /// A type without one is headed by its id capitalized, such as `Added`.
        #[arg(long = "heading", value_name = "TYPE=HEADING", value_parser = heading)]
        headings: Vec<(Type, String)>,
    },
}

/// Output selection flags shared by `changelog render` and `jp release render`.
// Skip generating an ArgGroup so subcommands that flatten Render do not collide
// with a group of the same name.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[group(skip)]
pub struct Render {
    /// Release version (e.g. `0.3.0`); if omitted, renders top-level fragments
    /// and any untagged release directories.
    #[arg(value_parser = version)]
    pub version: Option<Version>,

    /// `markdown` for rendered prose, or `json` for structured release data.
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// Markdown document to render: `notes` (default) for the release notes, or
    /// `upgrade` for the upgrade guide.
    #[arg(long, value_enum)]
    pub page: Option<Page>,

    /// Base URL or path for upgrade guide links in the release notes, such as
    /// `https://jp.computer/releases/0.3.0/upgrade`.
    ///
    /// Breaking entries link to `<upgrade-url>#<anchor>`.
    /// Defaults to `upgrade`.
    #[arg(long, value_name = "URL")]
    pub upgrade_url: Option<String>,
}

/// Output format for `render`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Markdown,
    Json,
}

/// Markdown document selected by `--page`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Page {
    Notes,
    Upgrade,
}

/// Validated output target for a [`Render`] invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// Structured JSON representation of the [`Release`].
    Json,

    /// Markdown release notes linking to the upgrade guide at `upgrade_guide`.
    Notes { upgrade_guide: String },

    /// Markdown upgrade guide.
    UpgradeGuide,
}

impl Render {
    /// Return the fragments directory [`Part`] selected by `self.version`.
    #[must_use]
    pub fn part(&self) -> Part {
        self.version.clone().map_or(Part::Unreleased, Part::Release)
    }

    /// Resolve the [`View`] to render, using `default` when `--format` is
    /// omitted.
    ///
    /// # Errors
    ///
    /// Returns an error if `--page` or `--upgrade-url` is combined with an
    /// incompatible format or page.
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
    /// Render `release` according to this view, using `headings` for changelog
    /// section titles.
    ///
    /// Returns an empty string when rendering [`View::UpgradeGuide`] for a
    /// release with no breaking entries; otherwise ends with a trailing
    /// newline.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
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

/// Parse a semantic version string (e.g. `0.3.0`), stripping an optional
/// leading `v`.
///
/// # Errors
///
/// Returns an error if `text` is not a valid semantic version.
pub fn version(text: &str) -> Result<Version, String> {
    let bare = text.strip_prefix('v').unwrap_or(text);

    Version::parse(bare)
        .map_err(|error| format!("`{text}` is not a release's version, such as `0.3.0`: {error}"))
}

/// Parse `--repository` as an HTTP(S) URL and strip any trailing `/`.
fn repository(text: &str) -> Result<String, String> {
    if !is_web_address(text) {
        return Err(format!(
            "`{text}` isn't a web address, such as `https://github.com/dcdpr/jp`"
        ));
    }

    Ok(text.trim_end_matches('/').to_owned())
}

/// Parse a `--heading` `TYPE=HEADING` argument.
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

/// Run the CLI with [`env::args_os`].
#[must_use]
pub fn main() -> ExitCode {
    run(env::args_os())
}

/// Run the CLI with `args` (including the binary name at index 0).
///
/// `--help`, `--version`, and parse errors exit the process directly via clap.
#[must_use]
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::parse_from(args).command {
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
            repository,
            headings,
        } => {
            let headings = Headings::from_iter(headings);
            let printed = render.view(Format::Markdown).and_then(|view| {
                let release = release::read(
                    &dir,
                    &render.part(),
                    forge,
                    repository.as_deref(),
                    &headings,
                )
                .map_err(|error| match error {
                    ReleaseError::NoAddress(error) => {
                        format!("{error}; name the project's with `--repository`")
                    }
                    error => error.to_string(),
                })?;
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
