//! The tools the assistant answers a review with, run when the host's `init`
//! carries a tool call.
//!
//! The host names a tool the way its `source` does: `reply` for `review_reply`,
//! and `comment` for `review_comment`.
//! A call is refused unless it comes from the review's conversation, and writes
//! only to the review, under its lock; a reply or comment is stored when the
//! call returns, so a turn stopped part way keeps what it wrote.
//! The host closed stdin after `init`, so nothing here asks it anything.
//!
//! A store inside the workspace is held to the call's access policy, as `jp
//! ticket`'s tools are held to theirs; a store outside it, such as the default
//! one in user-local storage, is written directly.

use camino::Utf8Path;
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Context as ToolContext, FsAccessError, Outcome};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::{
    anchor::{Anchor, Target},
    comments,
    id::{CommentRef, ReviewId},
    model::{Content, NewComment, Reply, Review, Role, Verdict},
    options::Options,
    paths, render, snapshot,
    store::{Blobs, Store, now},
    text::{Range, Text},
};

/// The tools this plugin serves, by the name a `source` gives them.
pub(crate) const TOOLS: &[&str] = &["comment", "reply"];

/// Answer one tool call.
///
/// Every failure becomes an error outcome, so the assistant reads what went
/// wrong and can correct the call.
pub(crate) fn run(init: &InitMessage, call: &ToolCall, options: &Options) -> Outcome {
    match answer(init, call, options) {
        Ok(content) => content.into(),
        Err(message) => Outcome::Error {
            message,
            trace: vec![],
            transient: true,
        },
    }
}

fn answer(init: &InitMessage, call: &ToolCall, options: &Options) -> Result<String, String> {
    if !TOOLS.contains(&call.name.as_str()) {
        return Err(format!(
            "`jp review` serves no tool named `{}`. It serves: {}.",
            call.name,
            TOOLS.join(", ")
        ));
    }
    if call.action == ToolAction::FormatArguments {
        return Err(format!(
            "`{}` has no preview of its own; style its parameters as `function_call` or `json` \
             rather than `tool`.",
            call.name
        ));
    }

    let root = init.workspace.root.as_path();
    let dir = options.store_dir(root, init.paths.user_workspace.as_deref())?;
    check_access(init, call, &dir)?;

    let writer = Writer {
        root,
        store: Store::new(dir),
        options,
        conversation: &call.conversation,
    };
    match call.name.as_str() {
        "reply" => writer.reply(&arguments(&call.arguments)?),
        _ => writer.comment(arguments(&call.arguments)?),
    }
}

/// Refuse a call whose access policy doesn't let it write to the review store,
/// when the store is inside the workspace.
///
/// A policy the plugin can't read refuses the call rather than reading as
/// unrestricted.
fn check_access(init: &InitMessage, call: &ToolCall, dir: &Utf8Path) -> Result<(), String> {
    let root = &init.workspace.root;
    let Ok(store) = dir.strip_prefix(root) else {
        return Ok(());
    };

    let access = call
        .access
        .clone()
        .map(serde_json::from_value::<AccessPolicy>)
        .transpose()
        .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;
    let context = ToolContext {
        root: root.clone(),
        action: Action::Run,
        access,
        workspace_id: init.workspace.id.clone(),
        conversation_id: call.conversation.clone(),
    };

    let refused =
        |error: FsAccessError| format!("`{}` can't write to the review store: {error}", call.name);
    context.check_read(store).map_err(refused)?;
    context.check_update(store).map_err(refused)?;
    // A comment can record a new version, whose contents are new blobs.
    if call.name == "comment" {
        context.check_create(store).map_err(refused)?;
    }

    Ok(())
}

/// A call's arguments, read into what its tool takes.
fn arguments<T: DeserializeOwned>(arguments: &Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(arguments.clone()))
        .map_err(|error| format!("The arguments don't fit the tool: {error}."))
}

/// What `review_reply` takes.
#[derive(Debug, Deserialize)]
struct ReplyArguments {
    comment: String,

    #[serde(default)]
    verdict: Option<Verdict>,

    body: String,
}

/// What `review_comment` takes.
#[derive(Debug, Deserialize)]
struct CommentArguments {
    review: String,

    #[serde(default)]
    path: Option<String>,

    #[serde(default)]
    lines: Option<String>,

    #[serde(default)]
    quote: Option<String>,

    body: String,
}

/// What a call writes with: the review store, and who it writes as.
struct Writer<'a> {
    /// The workspace root, which a review's file paths are relative to.
    root: &'a Utf8Path,

    store: Store,
    options: &'a Options,

    /// The conversation the call comes from, as JP prints it.
    conversation: &'a str,
}

impl Writer<'_> {
    /// Post the assistant's reply to a comment.
    fn reply(&self, args: &ReplyArguments) -> Result<String, String> {
        let comment: CommentRef = args.comment.parse()?;
        let reply = Reply {
            role: Role::Assistant,
            author: self.options.assistant.clone(),
            created_at: now(),
            body: bounded(&args.body, self.options.max_chars)?,
            verdict: args.verdict,
            pass: None,
        };

        let id = comment.review;
        self.store.update(id, |review, _| {
            admit(review, id, self.conversation)?;
            review
                .answer(comment.number, reply)
                .map_err(|refusal| refusal.message(id))
        })?;

        let verdict = args
            .verdict
            .map_or_else(String::new, |verdict| format!(", {verdict}"));
        Ok(format!("Replied to {comment}{verdict}."))
    }

    /// Open a comment of the assistant's.
    ///
    /// Its lines count in the working tree, which is recorded as a new version
    /// first when it changed since the latest.
    fn comment(&self, args: CommentArguments) -> Result<String, String> {
        let id: ReviewId = args.review.parse().map_err(|error| format!("{error}"))?;
        let body = bounded(&args.body, self.options.max_chars)?;
        let place = Place::read(args.path, args.lines.as_deref(), args.quote)?;
        let at = now();

        let (number, anchor) = self.store.update(id, |review, blobs| {
            admit(review, id, self.conversation)?;
            review.writable().map_err(|refusal| refusal.message(id))?;
            let version = snapshot::record(self.root, review, blobs, at)?;
            let anchor = Anchor {
                version,
                target: place.target(self.root, review, blobs, version)?,
            };
            let number = review
                .add_comment(NewComment {
                    role: Role::Assistant,
                    author: self.options.assistant.clone(),
                    created_at: at,
                    content: Content {
                        anchor: anchor.clone(),
                        body,
                        suggestion: None,
                    },
                })
                .map_err(|refusal| refusal.message(id))?;

            Ok::<_, String>((number, anchor))
        })?;

        Ok(format!(
            "Opened {} on {}.",
            CommentRef { review: id, number },
            render::written(&anchor)
        ))
    }
}

/// Refuse a call that doesn't come from the review's conversation, or from the
/// conversation of the pass it is running.
fn admit(review: &Review, id: ReviewId, conversation: &str) -> Result<(), String> {
    if review.passes.admits(conversation) {
        return Ok(());
    }

    let from = if conversation.is_empty() {
        "no conversation".to_owned()
    } else {
        conversation.to_owned()
    };

    Err(match &review.passes.conversation {
        Some(own) => format!("{id} runs its passes in {own}, and this call comes from {from}."),
        None => format!("{id} has no conversation yet; its first pass gives it one."),
    })
}

/// `body`, trimmed, refused when empty or longer than `max` characters.
fn bounded(body: &str, max: usize) -> Result<String, String> {
    let body = body.trim();
    let count = body.chars().count();
    if count == 0 {
        return Err("`body` is empty.".to_owned());
    }
    if count > max {
        return Err(format!(
            "`body` holds {count} characters, and a reply or comment holds at most {max}. Say it \
             in fewer words."
        ));
    }

    Ok(body.to_owned())
}

/// Where `review_comment` puts its comment, as its arguments name it.
struct Place {
    path: Option<String>,
    lines: Option<Range>,
    quote: Option<String>,
}

impl Place {
    /// Read the arguments naming the place: `lines` need a `path`, and a
    /// `quote` narrows `lines`.
    fn read(
        path: Option<String>,
        lines: Option<&str>,
        quote: Option<String>,
    ) -> Result<Self, String> {
        let lines = lines.map(whole_lines).transpose()?;
        match (&path, &lines, &quote) {
            (None, Some(_), _) => Err("`lines` needs `path`, the file they are in.".to_owned()),
            (_, None, Some(_)) => Err("`quote` narrows `lines`; give `lines` too.".to_owned()),
            _ => Ok(Self { path, lines, quote }),
        }
    }

    /// What the comment is on in `version` of `review`.
    fn target(
        self,
        root: &Utf8Path,
        review: &Review,
        blobs: &Blobs,
        version: u32,
    ) -> Result<Target, String> {
        let Some(typed) = self.path else {
            return Ok(Target::Review);
        };
        let path = paths::tracked(root, root, &review.paths, &typed).ok_or_else(|| {
            format!(
                "`{typed}` is not a file in the review, which tracks {}.",
                review.paths.join(", ")
            )
        })?;

        match (
            comments::target(review, blobs, version, path, self.lines)?,
            self.quote,
        ) {
            (Target::Range(path, lines), Some(quote)) => {
                let name = review.blob(version, &path).unwrap_or_default();
                let range = Text::new(&blobs.get(name)?).narrow(lines, &quote)?;
                Ok(Target::Range(path, range))
            }
            (target, _) => Ok(target),
        }
    }
}

/// `lines`, a line or a range of whole lines: `42` or `42-45`.
fn whole_lines(lines: &str) -> Result<Range, String> {
    match lines.parse::<Range>()? {
        whole @ Range::Lines { .. } => Ok(whole),
        Range::Span { .. } => Err(format!(
            "`lines` takes a line, `42`, or lines, `42-45`, not `{lines}`; narrow them to the \
             text you mean with `quote`."
        )),
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
