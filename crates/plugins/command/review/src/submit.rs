//! `jp review submit`: submit pending comments and replies as a pass and run
//! its delegated turn on the host.
//!
//! Holds the review's `.pass` lock ([`Store::lock_pass`]) across the entire
//! pass lifecycle:
//!
//! 1. Snapshot the working tree (if changed) and record the new pass.
//! 2. Send a `query` request to the host with the rendered pass prompt
//!    ([`crate::prompt`]) on the review's conversation (or a new conversation
//!    for the first pass).
//! 3. Poll the store while the turn runs and print each reply and comment
//!    written by the assistant's tools.
//! 4. Snapshot the working tree again to capture the assistant's file edits and
//!    record the pass outcome.

use std::{
    collections::HashSet,
    io::{BufRead, Write},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use doc::author;
use jp_plugin::{
    message::{
        HostToPlugin, InterruptRequest, PluginToHost, QueryCompleteResponse, QueryOutcome,
        QueryRequest,
    },
    stdio::{self, print_to},
};

use crate::{
    BINARY, Output,
    cmd::{Context, json, load},
    id::{Conversation, ReviewId},
    model::{Failure, Review, Role},
    pass::{Ended, NewPass, Outcome},
    prompt::{self, Prompt},
    render, snapshot,
    store::{Store, now},
    view::{self, PassReport},
};

/// How often the store is read for what the assistant wrote, while a turn runs.
const POLL: Duration = Duration::from_millis(250);

/// The correlation id a pass's `query` goes out with.
const QUERY_ID: &str = "pass";

/// What `jp review submit` asks for.
pub(crate) struct Request {
    pub id: ReviewId,

    /// The comments to send, by number; every comment waiting on the assistant
    /// when empty.
    pub only: Vec<u32>,

    pub message: Option<String>,

    /// The conversation to run in, as JP prints it.
    pub conversation: Option<String>,

    /// Configurations to run the turn under, after the `cfg` option's.
    pub cfg: Vec<String>,
}

impl Request {
    /// A pass of the review `id`; a message holding only whitespace is none.
    pub(crate) fn new(
        id: ReviewId,
        only: Vec<u32>,
        message: Option<String>,
        conversation: Option<Conversation>,
        cfg: Vec<String>,
    ) -> Self {
        Self {
            id,
            only,
            message: message
                .map(|message| message.trim().to_owned())
                .filter(|message| !message.is_empty()),
            conversation: conversation.map(|conversation| conversation.to_string()),
            cfg,
        }
    }
}

/// Run a pass: send what waits on the assistant, wait for the turn answering it
/// to end, and say how it went.
///
/// The host's messages arrive on `stdin`, and this plugin's go out on `stdout`.
pub(crate) fn submit(
    ctx: &Context<'_>,
    request: &Request,
    stdin: &mut impl BufRead,
    stdout: &mut (impl Write + Send),
) -> Result<Output, String> {
    let _pass = ctx
        .store
        .lock_pass(request.id)?
        .ok_or_else(|| held(request.id))?;
    let (number, review) = start(ctx, request)?;

    let out = Out(Mutex::new(stdout));
    let ending = converse(ctx, request, &review, number, stdin, &out);
    let review = finish(ctx, request.id, number, ending)?;

    report(ctx, request.id, &review, number)
}

/// Why a pass of the review `id` can't run: another process is running one.
pub(crate) fn held(id: ReviewId) -> String {
    format!("A pass of {id} is running; wait for it to end, or stop it where it runs.")
}

/// Record the working tree as a new version when it changed since the latest
/// one, then record the pass, returning the pass's number and the review.
///
/// The caller holds the review's `.pass` lock, so a pass that an earlier
/// process left without an outcome is recorded as interrupted first: that
/// process is gone.
pub(crate) fn start(ctx: &Context<'_>, request: &Request) -> Result<(u32, Review), Failure> {
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();
    let id = request.id;

    ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| Failure::Refused(id, refusal))?;
        review.passes.interrupt(at);
        let version = snapshot::record(&ctx.root, review, blobs, at)?;
        let number = review
            .submit(NewPass {
                author,
                created_at: at,
                version,
                only: request.only.clone(),
                message: request.message.clone(),
                conversation: request.conversation.clone(),
            })
            .map_err(|refusal| Failure::Refused(id, refusal))?;

        Ok((number, review.clone()))
    })
}

/// Ask the host to run the turn of pass `number`, and wait for it to end,
/// printing what the assistant writes to the store meanwhile.
fn converse<W: Write + Send>(
    ctx: &Context<'_>,
    request: &Request,
    review: &Review,
    number: u32,
    stdin: &mut impl BufRead,
    out: &Out<'_, W>,
) -> Ending {
    let content = match message(ctx, request.id, review, number) {
        Ok(content) => content,
        Err(error) => return Ending::failed(error),
    };
    let conversation = review
        .passes
        .get(number)
        .and_then(|pass| pass.conversation.clone());
    let query = query(
        ctx,
        request,
        review,
        conversation.as_deref(),
        QUERY_ID.to_owned(),
        content,
    );
    if let Err(error) = out.send(&PluginToHost::Query(query)) {
        return Ending::lost(error);
    }

    let text = !ctx.format.is_json();
    if let (true, Some(conversation)) = (text, &conversation) {
        out.say(&format!("Pass {number} on {conversation}.\n"));
    }
    let turn = Turn {
        store: &ctx.store,
        id: request.id,
        number,
        conversation,
        text,
    };
    if !text {
        return turn.wait(stdin, out);
    }

    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let watcher = scope.spawn(|| watch(&ctx.store, request.id, number, out, &done));
        let ending = turn.wait(stdin, out);

        done.store(true, Ordering::Release);
        watcher.thread().unpark();
        let mut printed = watcher.join().unwrap_or_default();
        printed.print_new(&ctx.store, request.id, number, out);

        ending
    })
}

/// The message of pass `number` of `review`.
pub(crate) fn message(
    ctx: &Context<'_>,
    id: ReviewId,
    review: &Review,
    number: u32,
) -> Result<String, String> {
    let pass = review
        .passes
        .get(number)
        .ok_or_else(|| format!("{id} has no pass {number}."))?;
    let carry = review.passes.carry(number);
    let texts = load(&ctx.store.blobs(id), prompt::blobs(review, pass, carry))?;

    Ok(prompt::message(&Prompt {
        id,
        review,
        pass,
        carry,
        texts: &texts,
    }))
}

/// The request, carrying the correlation id `id`, that runs a pass's turn on
/// `content`: in `conversation`, or in a new one named after the review.
pub(crate) fn query(
    ctx: &Context<'_>,
    request: &Request,
    review: &Review,
    conversation: Option<&str>,
    id: String,
    content: String,
) -> QueryRequest {
    QueryRequest {
        id: Some(id),
        conversation: conversation.unwrap_or_default().to_owned(),
        content,
        new: conversation.is_none(),
        title: conversation
            .is_none()
            .then(|| format!("Review {}: {}", request.id, review.title)),
        cfg: ctx
            .options
            .pass_cfg()
            .into_iter()
            .chain(request.cfg.iter().cloned())
            .collect(),
        schema: None,
        expires_in: None,
    }
}

/// Snapshot the working tree after pass `number` and record the pass outcome,
/// with the assistant's closing message when the host reported one, returning
/// the updated [`Review`].
///
/// If the working tree cannot be snapshotted, records `version: None` and
/// appends the snapshot error to the pass's error message.
pub(crate) fn finish(
    ctx: &Context<'_>,
    id: ReviewId,
    number: u32,
    ending: Ending,
) -> Result<Review, String> {
    let at = now();

    ctx.store.update(id, |review, blobs| {
        let (version, error) = match snapshot::record(&ctx.root, review, blobs, at) {
            Ok(version) => (Some(version), ending.error),
            Err(unread) => {
                let unread = format!("The files couldn't be recorded after the turn: {unread}");
                let error = match ending.error {
                    Some(error) => format!("{error}; {unread}"),
                    None => unread,
                };
                (None, Some(error))
            }
        };
        review.passes.end(number, Ended {
            outcome: ending.outcome,
            at,
            version,
            error,
            answer: ending.answer,
        });

        Ok::<_, String>(review.clone())
    })
}

/// What the command prints once the pass has ended: a summary, or a record
/// under `jp -F json`, and an error when the turn didn't run.
fn report(ctx: &Context<'_>, id: ReviewId, review: &Review, number: u32) -> Result<Output, String> {
    let report = view::pass_report(id, review, number)
        .ok_or_else(|| format!("{id} has no pass {number}."))?;
    let mut output = if ctx.format.is_json() {
        json(&report)?
    } else {
        Output::from(render::pass_end(&report))
    };
    output.failure = failure(&report);
    output.warnings.extend(warning(&report));

    Ok(output)
}

/// What went wrong with a pass whose turn ran, such as files that couldn't be
/// recorded after it.
fn warning(report: &PassReport<'_>) -> Option<String> {
    let ended = report.pass.ended.as_ref()?;
    match ended.outcome {
        Outcome::Completed | Outcome::Stopped => ended.error.clone(),
        Outcome::Failed | Outcome::Interrupted => None,
    }
}

/// The error a pass whose turn didn't run ends the command with.
fn failure(report: &PassReport<'_>) -> Option<String> {
    let ended = report.pass.ended.as_ref()?;
    let number = report.pass.number;
    let error = ended.error.as_deref().unwrap_or("no reason was given");

    match ended.outcome {
        Outcome::Failed => Some(format!("Pass {number} failed: {error}")),
        Outcome::Interrupted => Some(format!("Pass {number} was interrupted: {error}")),
        Outcome::Completed | Outcome::Stopped => None,
    }
}

/// How the turn of a pass ended, as far as this process knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ending {
    pub outcome: Outcome,
    pub error: Option<String>,

    /// The assistant's closing message, which a host from protocol 16 on
    /// reports when the turn holds one.
    pub answer: Option<String>,
}

impl Ending {
    /// The turn ended the way the host's `query_complete` says.
    pub(crate) fn of(done: QueryCompleteResponse) -> Self {
        let outcome = match done.outcome {
            QueryOutcome::Finished => Outcome::Completed,
            QueryOutcome::Stopped | QueryOutcome::Aborted => Outcome::Stopped,
        };

        Self {
            outcome,
            error: None,
            answer: done.answer,
        }
    }

    /// The host couldn't run the turn, or the turn failed.
    pub(crate) fn failed(error: String) -> Self {
        Self {
            outcome: Outcome::Failed,
            error: Some(error),
            answer: None,
        }
    }

    /// The host went away before saying how the turn ended.
    pub(crate) fn lost(error: String) -> Self {
        Self {
            outcome: Outcome::Interrupted,
            error: Some(error),
            answer: None,
        }
    }
}

/// A pass's turn, as the main thread waits on the host for it to end.
struct Turn<'a> {
    store: &'a Store,
    id: ReviewId,
    number: u32,

    /// The conversation the turn runs in, once it is known.
    conversation: Option<String>,

    /// Whether the command prints text, rather than one JSON record at the end.
    text: bool,
}

impl Turn<'_> {
    /// Read the host's messages until the turn ends.
    fn wait<W: Write>(mut self, stdin: &mut impl BufRead, out: &Out<'_, W>) -> Ending {
        loop {
            let message = match stdio::read(stdin, BINARY) {
                Ok(message) => message,
                Err(error) => return Ending::lost(error),
            };

            match message {
                HostToPlugin::Created(created) => self.created(&created.conversation, out),
                HostToPlugin::QueryComplete(done) => return Ending::of(done),
                HostToPlugin::Error(error) => return Ending::failed(error.message),
                HostToPlugin::Shutdown => self.stop(out),
                _ => {}
            }
        }
    }

    /// Record the conversation the host created for the pass, which the
    /// assistant's tool calls come from.
    fn created<W: Write>(&mut self, conversation: &str, out: &Out<'_, W>) {
        self.conversation = Some(conversation.to_owned());
        let number = self.number;
        let recorded = self.store.update(self.id, |review, _| {
            review.passes.created(number, conversation);
            Ok::<_, String>(())
        });

        match recorded {
            Ok(()) if self.text => out.say(&format!(
                "Pass {number} on a new conversation, {conversation}.\n"
            )),
            Ok(()) => {}
            Err(error) => out.warn(&format!(
                "{} couldn't record {conversation}, so the assistant's replies from it are \
                 refused: {error}",
                self.id
            )),
        }
    }

    /// Ask the host to stop the turn, which it is shutting down around; how it
    /// ended still arrives as the `query`'s answer.
    fn stop<W: Write>(&self, out: &Out<'_, W>) {
        let Some(conversation) = &self.conversation else {
            return;
        };

        // A host that can't be told has gone, which the next read reports.
        drop(out.send(&PluginToHost::Interrupt(InterruptRequest::stop(
            conversation.clone(),
        ))));
    }
}

/// Where this plugin's messages go out to the host, shared by the thread
/// waiting on the turn and the one watching the store.
struct Out<'a, W>(Mutex<&'a mut W>);

impl<W: Write> Out<'_, W> {
    fn send(&self, message: &PluginToHost) -> Result<(), String> {
        let mut stdout = self
            .0
            .lock()
            .map_err(|_| "a thread writing to the host panicked".to_owned())?;

        stdio::send(&mut **stdout, message)
    }

    /// Print `text` as the command's output.
    ///
    /// A host that can't be written to has gone, which the next read reports.
    fn say(&self, text: &str) {
        self.print(text, "content");
    }

    /// Print a warning, beside the command's output.
    fn warn(&self, text: &str) {
        self.print(&format!("Warning: {text}\n"), "chrome");
    }

    fn print(&self, text: &str, channel: &str) {
        if let Ok(mut stdout) = self.0.lock() {
            drop(print_to(&mut **stdout, text, channel));
        }
    }
}

/// Print what the assistant writes during pass `number` as it lands in the
/// store, until `done`.
fn watch<W: Write>(
    store: &Store,
    id: ReviewId,
    number: u32,
    out: &Out<'_, W>,
    done: &AtomicBool,
) -> Printed {
    let mut printed = Printed::default();
    while !done.load(Ordering::Acquire) {
        printed.print_new(store, id, number, out);
        thread::park_timeout(POLL);
    }

    printed
}

/// What `jp review submit` has printed of what the assistant wrote during its
/// pass.
#[derive(Debug, Default)]
struct Printed {
    /// Replies, by the comment they answer and their place among its replies.
    replies: HashSet<(u32, usize)>,

    /// Comments the assistant opened, by number.
    opened: HashSet<u32>,
}

impl Printed {
    /// Print what the store holds of pass `number` that isn't printed yet.
    fn print_new<W: Write>(&mut self, store: &Store, id: ReviewId, number: u32, out: &Out<'_, W>) {
        // A read that fails leaves the lines to the next one.
        let Ok(review) = store.peek(id) else {
            return;
        };

        for line in self.new_lines(&review, number) {
            out.say(&line);
        }
    }

    /// The lines for what the assistant wrote during pass `number` that aren't
    /// printed yet, which are then taken as printed.
    fn new_lines(&mut self, review: &Review, number: u32) -> Vec<String> {
        let mut lines = vec![];
        for comment in &review.comments {
            let opened = comment.role == Role::Assistant && comment.pass == Some(number);
            if opened && self.opened.insert(comment.number) {
                lines.push(render::opened_line(comment));
            }

            for (index, reply) in comment.replies.iter().enumerate() {
                let written = reply.role == Role::Assistant && reply.pass == Some(number);
                if written && self.replies.insert((comment.number, index)) {
                    lines.push(render::reply_line(comment.number, reply));
                }
            }
        }

        lines
    }
}

#[cfg(test)]
#[path = "submit_tests.rs"]
mod tests;
