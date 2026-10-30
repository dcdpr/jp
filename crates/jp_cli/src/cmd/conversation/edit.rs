use std::{fmt::Write as _, fs, str::FromStr, time::Duration};

use camino::Utf8PathBuf;
use chrono::Utc;
use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches, Command,
    builder::{BoolValueParser, NonEmptyStringValueParser, TypedValueParser},
};
use crossterm::style::Stylize as _;
use inquire::Confirm;
use jp_config::PartialAppConfig;
use jp_conversation::ConversationId;
use jp_editor::{EditOutcome, EditRequest};
use jp_storage::{
    LoadError,
    backend::{FsStorageBackend, LoadBackend, Projection},
};
use jp_workspace::ConversationHandle;

use super::{path::resolve_paths, title};
use crate::{
    cli::{
        CommandArgs,
        flag::{Flag, FlagValues, OptionalValue, flag_arg},
    },
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    editor::build_editor_backend,
    error::Error,
};

/// `jp conversation edit`.
///
/// Either changes properties of the conversation (`--local`, `--pin`, `--tmp`,
/// `--title`, and their negations), or opens its files in `$EDITOR`; never
/// both.
#[derive(Debug)]
pub(crate) struct Edit {
    target: PositionalIds<true, true>,

    /// `--local[=BOOL]`; `Some(None)` toggles.
    local: Option<Option<bool>>,

    /// `--pin[=BOOL]`; `Some(None)` toggles.
    pin: Option<Option<bool>>,

    /// `--tmp[=DURATION]`; `Some(None)` toggles.
    expires_at: Option<Option<ExpirationDuration>>,

    /// `--no-tmp`.
    no_expires_at: bool,

    /// `--title[=TITLE]`; `Some(None)` generates one.
    title: Option<Option<String>>,

    /// `--no-title`.
    no_title: bool,

    /// Open `events.json`.
    events: bool,

    /// Open `metadata.json`.
    metadata: bool,

    /// Open `base_config.json`.
    base_config: bool,
}

/// The group of flags that change a property of the conversation.
const PROPERTY: &str = "property";

/// The group of flags that name a file to open.
const FILE: &str = "file";

impl CommandArgs for Edit {
    fn augment(cmd: Command) -> Command {
        let file = |id: &'static str, short: char, long: &'static str, help: &'static str| {
            Arg::new(id)
                .short(short)
                .long(long)
                .action(ArgAction::SetTrue)
                .group(FILE)
                .conflicts_with(PROPERTY)
                .help(help)
        };

        PositionalIds::<true, true>::augment(cmd)
            .group(ArgGroup::new(PROPERTY).multiple(false))
            .group(ArgGroup::new(FILE).multiple(false))
            .arg(flag_arg::<Local>())
            .arg(flag_arg::<Pin>())
            .arg(flag_arg::<Expiry>())
            .arg(
                Arg::new("no_expires_at")
                    .long("no-tmp")
                    .action(ArgAction::SetTrue)
                    .group(PROPERTY)
                    .help("Remove the expiration time of the conversation"),
            )
            .arg(flag_arg::<Title>())
            .arg(
                Arg::new("no_title")
                    .long("no-title")
                    .action(ArgAction::SetTrue)
                    .group(PROPERTY)
                    .help("Remove the title of the conversation"),
            )
            .arg(
                file("events", 'e', "events", "Open `events.json` in `$EDITOR`").long_help(
                    "Open `events.json` in `$EDITOR`.\n\nEach entry carries an `event_id` \
                     identifying it. Keep it when you edit an entry's content, and drop it when \
                     you add one: a missing, empty, or duplicated ID is replaced on the next \
                     load.\n\nA `turn_start` entry can also carry a `summary` of its turn. Remove \
                     it when you change the turn's content: a stored summary is never rewritten, \
                     and a turn without one is summarized again by a later `jp query` while \
                     `conversation.turn_summary.generate.auto` is on.",
                ),
            )
            .arg(file(
                "metadata",
                'm',
                "metadata",
                "Open `metadata.json` in `$EDITOR`",
            ))
            .arg(file(
                "base_config",
                'b',
                "base-config",
                "Open `base_config.json` in `$EDITOR`",
            ))
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: PositionalIds::from_matches(matches)?,
            local: Flag::<Local>::strict(matches)?,
            pin: Flag::<Pin>::strict(matches)?,
            expires_at: Flag::<Expiry>::strict(matches)?,
            no_expires_at: matches.get_flag("no_expires_at"),
            title: Flag::<Title>::strict(matches)?,
            no_title: matches.get_flag("no_title"),
            events: matches.get_flag("events"),
            metadata: matches.get_flag("metadata"),
            base_config: matches.get_flag("base_config"),
        })
    }
}

/// `--local[=BOOL]`: keep the conversation out of the workspace, or toggle it.
enum Local {}

impl OptionalValue for Local {
    const ID: &'static str = "local";

    type Context = ();
    type Output = Option<Option<bool>>;
    type Value = bool;

    fn declare(arg: Arg) -> Arg {
        arg.long("local")
            .value_name("LOCAL")
            .group(PROPERTY)
            .help("Toggle whether the conversation is kept local to your machine")
            .long_help(
                "Toggle whether the conversation is kept local to your machine.\n\nEvery \
                 conversation is stored durably on your machine. A non-local conversation is \
                 additionally projected into the workspace's `.jp/conversations/` directory so it \
                 can be committed to version control; a local conversation is not, keeping it out \
                 of version control. Without a value, toggles the current state.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = bool> {
        BoolValueParser::new()
    }

    fn collect(values: FlagValues<bool>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

/// `--pin[=BOOL]`: pin or unpin the conversation, or toggle it.
enum Pin {}

impl OptionalValue for Pin {
    const ID: &'static str = "pin";

    type Context = ();
    type Output = Option<Option<bool>>;
    type Value = bool;

    fn declare(arg: Arg) -> Arg {
        arg.long("pin")
            .value_name("PIN")
            .group(PROPERTY)
            .help("Toggle pinning of the conversation")
            .long_help(
                "Toggle pinning of the conversation.\n\nPinned conversations are displayed \
                 prominently in listings and pickers. Without a value, toggles the current pin \
                 state.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = bool> {
        BoolValueParser::new()
    }

    fn collect(values: FlagValues<bool>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

/// `--tmp[=DURATION]`: set when the conversation expires, or toggle it.
enum Expiry {}

impl OptionalValue for Expiry {
    const ID: &'static str = "expires_at";

    type Context = ();
    type Output = Option<Option<ExpirationDuration>>;
    type Value = ExpirationDuration;

    fn declare(arg: Arg) -> Arg {
        arg.long("tmp")
            .value_name("EXPIRES_AT")
            .group(PROPERTY)
            .conflicts_with("no_expires_at")
            .help("Toggle or set the expiration time of the conversation")
            .long_help(
                "Toggle or set the expiration time of the conversation.\n\nWithout a value, \
                 toggles: removes expiration if set, or sets it to expire immediately (when no \
                 longer active) if unset.\n\nAccepts a duration (e.g. `1h`, `30m`) or `now` for \
                 immediate expiration.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = ExpirationDuration> {
        parse_expiration
    }

    fn collect(values: FlagValues<ExpirationDuration>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

/// `--title[=TITLE]`: set the title, or generate one.
enum Title {}

impl OptionalValue for Title {
    const ID: &'static str = "title";

    type Context = ();
    type Output = Option<Option<String>>;
    type Value = String;

    fn declare(arg: Arg) -> Arg {
        arg.long("title")
            .value_name("TITLE")
            .group(PROPERTY)
            .conflicts_with("no_title")
            .help("Set the title of the conversation")
            .long_help(
                "Set the title of the conversation.\n\nWithout a value, generates one with an LLM \
                 and offers the candidates as a picker — the same as `conversation title`, which \
                 additionally exposes the model and candidate count.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = String> {
        NonEmptyStringValueParser::new()
    }

    fn collect(values: FlagValues<String>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

impl Edit {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        if self.has_property_flags() {
            return self.run_property_edit(ctx, handles).await;
        }

        self.run_open_editor(ctx, &handles)
    }

    /// Whether any metadata mutation flag is set.
    fn has_property_flags(&self) -> bool {
        self.local.is_some()
            || self.pin.is_some()
            || self.expires_at.is_some()
            || self.no_expires_at
            || self.title.is_some()
            || self.no_title
    }

    /// Open the conversation directory or specific files in `$EDITOR`.
    ///
    /// After the editor exits, the edited files are validated by loading them
    /// back exactly as the next startup would.
    /// A malformed edit is never committed: the parse error is printed, and the
    /// user is asked whether to re-open the editor to fix it or discard the
    /// edit (restoring the original files).
    /// In non-interactive mode an invalid edit is discarded with an error
    /// rather than prompting.
    ///
    /// A non-interactive invocation never opens the editor at all: the editor
    /// is a child process that exits when a person saves and closes it, so with
    /// nobody there it blocks forever on a buffer no one will close.
    /// There is nothing else this command can do, so it fails instead.
    fn run_open_editor(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        let config = ctx.config();
        let editor = match build_editor_backend(&config.editor, &ctx.printer) {
            Some(editor) if ctx.term.interactive => editor,
            None => return Err(Error::MissingEditor.into()),
            Some(_) => {
                return Err(Error::NonInteractiveEditor {
                    suggestion: "Run this from a terminal, or edit the files directly at the path \
                                 from `jp conversation path`.",
                }
                .into());
            }
        };

        let fs = ctx
            .fs_backend
            .clone()
            .ok_or("no filesystem storage configured")?;

        let ids: Vec<_> = handles.iter().map(ConversationHandle::id).collect();
        let mut paths = Vec::new();
        for id in &ids {
            paths.extend(resolve_paths(
                &fs,
                id,
                self.events,
                self.metadata,
                self.base_config,
            )?);
        }

        // Snapshot the pre-edit content so an edit that fails to load can be
        // reverted, leaving the conversation untouched.
        let snapshots: Vec<(Utf8PathBuf, Option<String>)> = paths
            .iter()
            .map(|path| (path.clone(), fs::read_to_string(path).ok()))
            .collect();

        loop {
            let outcome = editor
                .edit_file(EditRequest {
                    paths: &paths,
                    cwd: None,
                })
                .map_err(|error| Error::Editor(error.to_string()))?;

            if outcome == EditOutcome::Cancelled {
                // The editor aborted (non-zero exit). Discard any partial edits
                // and leave the conversation unchanged.
                Self::restore_snapshots(&snapshots)?;
                return Err(Error::Editor(
                    "the editor was cancelled; the conversation is unchanged".to_owned(),
                )
                .into());
            }

            let Err(error) = self.validate_edits(&fs, &ids) else {
                break;
            };

            // The edit doesn't load back (e.g. malformed JSON). Show the error
            // and let the user fix or discard it — never commit a broken edit.
            let _ = writeln!(
                ctx.printer.prompt_writer(),
                "{}",
                format!("The edited file could not be loaded back:\n\n{error}\n").red()
            );

            if ctx.term.interactive
                && Confirm::new("Re-open the editor to fix it?")
                    .with_default(true)
                    .prompt()
                    .unwrap_or(false)
            {
                continue;
            }

            Self::restore_snapshots(&snapshots)?;
            return Err(Error::Editor(
                "the edit could not be loaded and was discarded; the conversation is unchanged"
                    .to_owned(),
            )
            .into());
        }

        // A managed editor command edits the workspace copy of a projected
        // conversation; re-sync the user-local copy so both roots stay
        // consistent immediately rather than awaiting lazy reconciliation.
        for id in &ids {
            fs.sync_projection(id)?;
        }

        Ok(())
    }

    /// Validate edited files by loading them back exactly as the next startup
    /// would, so a successful return means the edit will not be sanitized away.
    fn validate_edits(
        &self,
        fs: &FsStorageBackend,
        ids: &[ConversationId],
    ) -> Result<(), LoadError> {
        for id in ids {
            if self.events || self.base_config {
                // No fallback: a file the user just edited has to stand on its
                // own, rather than pass validation on values borrowed from the
                // workspace config.
                fs.load_conversation_stream(id, &PartialAppConfig::empty())?;
            }
            if self.metadata {
                fs.load_conversation_metadata(id)?;
            }
        }
        Ok(())
    }

    /// Restore files to their pre-edit content, removing ones that were absent.
    fn restore_snapshots(snapshots: &[(Utf8PathBuf, Option<String>)]) -> Output {
        for (path, content) in snapshots {
            match content {
                Some(content) => fs::write(path, content.as_bytes())?,
                None if path.exists() => fs::remove_file(path)?,
                None => {}
            }
        }
        Ok(())
    }

    /// Mutate conversation metadata properties.
    async fn run_property_edit(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for handle in handles {
            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
                LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
            };

            let mut conv = lock.into_mut();
            if let Some(local) = self.local {
                let projection = match local {
                    Some(true) => Projection::LocalOnly,
                    Some(false) => Projection::Projected,
                    None => match conv.projection() {
                        Projection::LocalOnly => Projection::Projected,
                        Projection::Projected => Projection::LocalOnly,
                    },
                };
                conv.set_projection(projection);
            }

            if let Some(pinned) = self.pin {
                conv.update_metadata(|m| {
                    let pin = pinned.unwrap_or(!m.is_pinned());
                    m.pinned_at = if pin { Some(Utc::now()) } else { None };
                });
            }

            if let Some(ref new_title) = self.title {
                // A bare `--title` asks for a generated one; `conversation
                // title` owns that path and its flags.
                let title = if let Some(title) = new_title {
                    title.clone()
                } else {
                    let cfg = ctx.config();
                    let events = conv.events().clone();
                    title::select(ctx, &cfg, events, title::DEFAULT_COUNT, false).await?
                };

                conv.update_metadata(|m| m.title = Some(title));
            } else if self.no_title {
                conv.update_metadata(|m| m.title = None);
            }

            if let Some(ephemeral) = self.expires_at {
                conv.update_metadata(|m| match ephemeral {
                    Some(dur) => m.expires_at = Some(Utc::now() + dur.0),
                    None => {
                        // Toggle: remove if set, expire now if unset.
                        if m.expires_at.is_some() {
                            m.expires_at = None;
                        } else {
                            m.expires_at = Some(Utc::now());
                        }
                    }
                });
            } else if self.no_expires_at {
                conv.update_metadata(|m| m.expires_at = None);
            }

            // Write before reporting success, so a failed write is an error
            // rather than a confirmation the user cannot trust.
            conv.flush()?;
        }

        ctx.printer.println("Conversation(s) updated.");
        Ok(())
    }
}

/// Parse the `--tmp` duration.
fn parse_expiration(value: &str) -> std::result::Result<ExpirationDuration, String> {
    value.parse()
}

/// Duration value for `--tmp`, supporting `now` as an alias for zero duration.
#[derive(Debug, Clone, Copy)]
struct ExpirationDuration(Duration);

impl FromStr for ExpirationDuration {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("now") {
            Ok(Self(Duration::ZERO))
        } else {
            humantime::parse_duration(s)
                .map(Self)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
