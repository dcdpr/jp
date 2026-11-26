//! Composing the request a query sends: the query text, piped stdin, a replayed
//! or quoted message, the editor, and `--template`.
//!
//! [`Query::build_conversation`] is the entry point.
//! It reads the conversation stream without changing it: any events a replay
//! removes are described by a [`PendingStreamTrim`] and removed when the turn
//! starts.

use std::borrow::Cow;

use camino::Utf8Path;
use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::{ConversationEvent, ConversationStream, event::ChatRequest};
use jp_printer::Printer;
use minijinja::{Environment, UndefinedBehavior};
use tracing::{debug, warn};

use super::{Query, quote::seed_quoted_reply};
use crate::{
    editor,
    error::{Error, Result},
};

impl Query {
    /// Build the chat request for this query.
    ///
    /// Returns the request's [`QuerySource`], any editor-provided config, the
    /// stream edits deferred to the turn-start commit point, and the
    /// [`ChatRequest`], if non-empty.
    /// The stream is **not** mutated here: the request is added — and any
    /// pending replay trim applied — by [`TurnCoordinator::start_turn`].
    ///
    /// [`TurnCoordinator::start_turn`]: super::turn::TurnCoordinator::start_turn
    pub(super) fn build_conversation(
        &self,
        piped: &str,
        query: Option<&str>,
        stream: &ConversationStream,
        interactive: bool,
        config: &AppConfig,
        conversation_root: &Utf8Path,
        printer: &Printer,
    ) -> Result<BuiltConversation> {
        let mut pending_trim = PendingStreamTrim::default();

        // If replaying, the conversation's last turn is logically removed and
        // the request that opened it is replayed. The removal is
        // computed on a clone (`view`) and only committed to the durable
        // stream at the turn-start commit point (see [`PendingStreamTrim`]):
        // an empty query, an interrupt, or an error before the turn starts
        // leaves the persisted stream untouched.
        //
        // If not replaying (or replaying but no chat request event exists), we
        // create a new `ChatRequest` event, to populate with either the
        // provided query, or the contents of the text editor.
        let (view, mut chat_request) = if self.replay {
            pending_trim.replay_turn = true;
            let mut view = stream.clone();
            let chat_request = view.trim_last_turn().unwrap_or_default();
            (Cow::Owned(view), chat_request)
        } else {
            (Cow::Borrowed(stream), ChatRequest::default())
        };
        let view = view.as_ref();

        // If stdin contained data, we prepend it to the chat request.
        if !piped.is_empty() {
            let sep = if chat_request.is_empty() { "" } else { "\n\n" };
            *chat_request = format!("{piped}{sep}{chat_request}");
        }

        // If a query is provided, prepend it to the chat request. This is only
        // relevant for replays, otherwise the chat request is still empty, so
        // we replace it with the provided query.
        if let Some(text) = query {
            let sep = if chat_request.is_empty() { "" } else { "\n\n" };
            *chat_request = format!("{text}{sep}{chat_request}");
        }

        // If --quote is set, prepend the last assistant message so it sits at
        // the top of the editor buffer. The user can then intersperse replies
        // between the quoted lines (mutt-style inline reply). Missing message
        // (e.g. brand new conversation) degrades to a warning and the editor
        // opens with whatever else was seeded.
        if let Some(prefixed) = self.quote
            && !seed_quoted_reply(&mut chat_request, view, prefixed, config)
        {
            warn!("--quote: no prior assistant message in this conversation");
        }

        let (query_source, editor_provided_config) = self.edit_message(
            &mut chat_request,
            view,
            &mut pending_trim,
            !piped.is_empty(),
            interactive,
            config,
            conversation_root,
            printer,
        )?;

        if self.template {
            let mut env = Environment::empty();
            env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
            env.add_template("query", &chat_request.content)?;

            let tmpl = env.get_template("query")?;
            // TODO: supported nested variables
            for var in tmpl.undeclared_variables(false) {
                if config.template.values.contains_key(&var) {
                    continue;
                }

                return Err(Error::TemplateUndefinedVariable(var));
            }

            *chat_request = tmpl.render(&config.template.values)?;
        }

        Ok(BuiltConversation {
            query_source,
            editor_provided_config,
            pending_trim,
            chat_request: (!chat_request.is_empty()).then_some(chat_request),
        })
    }

    // Open the editor for the query, if requested.
    //
    // `interactive` says whether a user is present to close the editor; when
    // they are not, the editor is never opened.
    fn edit_message(
        &self,
        request: &mut ChatRequest,
        stream: &ConversationStream,
        pending_trim: &mut PendingStreamTrim,
        piped: bool,
        interactive: bool,
        config: &AppConfig,
        conversation_root: &Utf8Path,
        printer: &Printer,
    ) -> Result<(QuerySource, PartialAppConfig)> {
        // If there is no query provided, but the user explicitly requested not
        // to open the editor, we populate the query with a default message,
        // since most LLM providers do not support empty queries.
        //
        // See `force_no_edit` why this can be useful.
        if request.is_empty() && self.force_no_edit() {
            // If the last event in the stream is a `ChatRequest`, we don't add
            // anything, and simply "replay" the last message in the
            // conversation. The event itself is only removed at the turn-start
            // commit point (the request re-enters the stream as part of the
            // new turn); until then the persisted stream stays untouched.
            //
            // Otherwise we add a default "continue" message.
            let last_request = stream
                .last_turn_event()
                .and_then(ConversationEvent::as_chat_request);

            if let Some(req) = last_request {
                *request = req.clone();
                pending_trim.pop_request = true;
            } else {
                "continue".clone_into(request);
            }

            // Nothing below applies: the editor is suppressed and the request
            // is fully synthesized, so report that to the caller (which echoes
            // it — the user never typed or saw this text).
            return Ok((QuerySource::Synthesized, PartialAppConfig::empty()));
        }

        // The remaining non-editor paths send the request text as-is. With
        // `--quote` that text was assembled from the quoted assistant message
        // rather than typed by the user, so it is synthesized too (and
        // therefore echoed); anything else was typed or piped inline.
        let source = if self.quote.is_some() {
            QuerySource::Synthesized
        } else {
            QuerySource::Inline
        };

        // If a query is provided, and editing is not explicitly requested, or
        // in addition to the query, stdin contains data, or editing was
        // explicitly suppressed with `--no-edit`, we omit opening the editor.
        if (self.query.as_ref().is_some_and(|v| !v.is_empty()) || !piped || self.force_no_edit())
            && !self.force_edit()
            && !request.is_empty()
        {
            return Ok((source, PartialAppConfig::empty()));
        }

        // The editor is a child process that paints the whole screen and
        // exits when a person saves and closes it. With nobody there it blocks
        // forever on a buffer no one will close, so a non-interactive
        // invocation is treated like having no editor configured: send whatever
        // the request already holds, and fail when it holds nothing. The one
        // exception is a caller that asked for the editor by name, below.
        let backend = match editor::build_editor_backend(&config.editor, printer) {
            Some(backend) if interactive => backend,
            // `--edit` and `--quote` ask for the editor explicitly, so the
            // seeded text was never meant to be sent unread: with `--quote` it
            // is the assistant's own last message, waiting for a reply between
            // the lines.
            Some(_) if self.force_edit() => {
                return Err(Error::NonInteractiveEditor {
                    // Without text there is nothing to "send as-is": `--no-edit`
                    // on an empty request replays the trailing request or sends
                    // `continue`, the same fallback the empty arm below names.
                    suggestion: if request.is_empty() {
                        "Run this from a terminal, or use --no-edit to send a placeholder message."
                    } else {
                        "Run this from a terminal, or pass --no-edit instead to send the text \
                         as-is."
                    },
                });
            }
            backend if !request.is_empty() => {
                if backend.is_some() {
                    debug!("Not opening the editor: the invocation is non-interactive.");
                }
                return Ok((source, PartialAppConfig::empty()));
            }
            None => return Err(Error::MissingEditor),
            Some(_) => {
                return Err(Error::NonInteractiveEditor {
                    suggestion: "Pass the query as an argument or on stdin, or use --no-edit to \
                                 send a placeholder message.",
                });
            }
        };

        let (content, editor_provided_config) = editor::edit_query(
            config,
            conversation_root,
            stream,
            request.as_str(),
            backend.as_ref(),
            None,
        )?;
        request.content = content;

        Ok((QuerySource::Editor, editor_provided_config))
    }

    /// Whether the chat request should be echoed to the terminal before the
    /// turn starts.
    ///
    /// Echo when the user can't already see what's being sent: a query composed
    /// in an editor (the editor took over the screen), a request synthesized by
    /// `--no-edit` or `--quote` (the user never typed or saw the final text),
    /// or a `--replay` (the message is pulled from history, not typed on the
    /// command line).
    /// A plain inline query needs no echo — the user just typed it.
    pub(super) fn should_echo_request(&self, source: QuerySource) -> bool {
        source != QuerySource::Inline || self.replay
    }

    /// Returns `true` if editing is explicitly disabled.
    ///
    /// This signals that even if no query is provided, no editor should be
    /// opened, but instead an empty query should be used.
    ///
    /// This can be used for example when requesting a tool call without needing
    /// additional context to be provided.
    fn force_no_edit(&self) -> bool {
        self.no_edit
    }

    /// Returns `true` if editing is explicitly enabled.
    ///
    /// This means the `--edit` flag was provided (and not `--no-edit`), or
    /// `--quote` was provided (which implies editing).
    /// In either case the editor should be opened, regardless of whether a
    /// query is provided as an argument.
    fn force_edit(&self) -> bool {
        !self.force_no_edit() && (self.edit || self.quote.is_some())
    }
}

/// Where the outgoing chat request's content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuerySource {
    /// Composed in the interactive editor.
    Editor,

    /// Provided inline: as a command-line argument, piped on stdin, or a
    /// replayed message — anything already final without an editor round-trip.
    Inline,

    /// Synthesized without the user typing or seeing the final text:
    /// `--no-edit` was given without a query (the conversation's trailing
    /// request is re-sent, or a default "continue" message is used), or
    /// `--quote` assembled the request from the quoted assistant message
    /// without an editor round-trip.
    Synthesized,
}

/// Output of [`Query::build_conversation`].
pub(super) struct BuiltConversation {
    /// Where the request's content came from.
    pub(super) query_source: QuerySource,
    /// Config assignments parsed from the editor buffer's front matter.
    pub(super) editor_provided_config: PartialAppConfig,
    /// Stream edits deferred to the turn-start commit point.
    pub(super) pending_trim: PendingStreamTrim,
    /// The composed request, if non-empty.
    pub(super) chat_request: Option<ChatRequest>,
}

/// Destructive stream edits computed while building a request, deferred to the
/// turn-start commit point.
///
/// `--replay` (and the bare `--no-edit` replay-last-request path) logically
/// removes trailing events before re-sending a request.
/// Applying that removal eagerly would persist a stream missing its last turn
/// while the editor is open and during MCP boot / attachment loading — an
/// interrupt (or an empty query) in that window would make the removal
/// permanent without the replacement request ever landing.
/// Instead the removal is described here and applied inside the same
/// `update_events` scope that appends the new turn
/// ([`TurnCoordinator::start_turn`] via [`run_turn_loop`]), so the persisted
/// stream goes from "old turn present" to "old turn replaced" in a single
/// write.
///
/// [`TurnCoordinator::start_turn`]: super::turn::TurnCoordinator::start_turn
/// [`run_turn_loop`]: super::turn_loop::run_turn_loop
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PendingStreamTrim {
    /// Remove the conversation's last turn (`--replay`).
    replay_turn: bool,
    /// Remove the trailing [`ChatRequest`] event (bare `--no-edit` replaying
    /// the last request).
    pop_request: bool,
}

impl PendingStreamTrim {
    /// Apply the deferred removals to the stream.
    pub(crate) fn apply(self, stream: &mut ConversationStream) {
        if self.replay_turn {
            drop(stream.trim_last_turn());
        }
        if self.pop_request {
            drop(stream.pop_if(ConversationEvent::is_chat_request));
        }

        // A trim can leave a [`TurnStart`] with nothing after it: popping a
        // turn's only request leaves the marker that opened the turn. Remove it
        // so the replacement request doesn't open a second turn right behind
        // it — the later stream sanitization only collapses duplicate
        // `TurnStart`s before the *first* request, so a stale middle marker
        // would survive in multi-turn conversations.
        //
        // Gated on a trim having run so a default (no-op) `PendingStreamTrim`
        // never mutates the stream.
        //
        // [`TurnStart`]: jp_conversation::event::TurnStart
        if self.replay_turn || self.pop_request {
            stream.trim_trailing_empty_turn();
        }
    }
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
