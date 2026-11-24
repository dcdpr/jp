//! Transport-independent handlers for the `jp review serve` JSON HTTP API.
//!
//! Each endpoint function takes a parsed request and returns a [`Reply`] or an
//! [`ApiError`] with an HTTP status code and `{"error": "..."}` body.
//! Requests and answers are `jp_review_api`'s types, which a browser client
//! shares.
//! Where an endpoint corresponds to a CLI command, the response matches the
//! JSON emitted by `jp -F json review`.
//! All handlers perform synchronous store I/O and are run on blocking threads
//! by `server`.
//!
//! For browser clients, [`AnchorRequest`] accepts UTF-8 byte offsets (`bytes`)
//! in addition to line/column ranges, and projected anchors include byte
//! offsets alongside line/column ranges.

use axum::http::StatusCode;
use camino::{Utf8Path, Utf8PathBuf};
use doc::{author, heading::headings};
use jp_plugin::message::OutputFormat;
use jp_review_api::{
    AnchorRequest, DetailQuery, DetailReply, DiffQuery, EditRequest, ErrorBody, FileQuery,
    FileReply, Format, Heading, Listing, NewCommentRequest, NewReview, PassRequest, ReadQuery,
    Recorded, ReplyRequest, Resolved, Running, Unapproved, Unread, Unreadable, Withdrawal,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    anchor::{Anchor, Target},
    cmd::{self, Context, load, to_json},
    comments,
    id::{CommentRef, Conversation, ReviewId},
    model::{Content, Edit, Failure, NewComment, Refusal, Review, Role, Why},
    options::Options,
    pass::{PassRef, Unspanned},
    progress::ReadUntil,
    reading, snapshot,
    store::{Blobs, Store, now},
    submit::{self, Request},
    text::{Range, Text},
    view::{self, Texts},
};

/// Shared workspace state for HTTP API handlers.
pub(crate) struct Workspace {
    pub root: Utf8PathBuf,
    pub config: Value,
    pub options: Options,
    pub store: Store,
}

impl Workspace {
    /// The workspace's name: its root directory's, or the whole root when that
    /// has none.
    pub(crate) fn name(&self) -> &str {
        self.root.file_name().unwrap_or(self.root.as_str())
    }

    /// Build a command [`Context`] rooted at the workspace root with JSON
    /// output.
    pub(crate) fn context(&self) -> Context<'_> {
        Context {
            root: self.root.clone(),
            cwd: self.root.clone(),
            config: &self.config,
            options: &self.options,
            store: self.store.clone(),
            format: OutputFormat::Json,
        }
    }
}

/// Successful HTTP API response: status code and serialized JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub status: StatusCode,
    pub json: String,
}

impl Reply {
    /// Serialize `value` as a JSON response with `status`.
    // Do not add an `ok` constructor: rustqual matches `.ok()` calls with
    // uninferred receiver types (such as `env::var(..).ok()`) against any
    // inherent method named `ok` in the workspace, reclassifying those callers
    // as `Integration` and flagging their `unsafe` blocks as `CX-006`.
    pub(crate) fn new(status: StatusCode, value: &impl Serialize) -> Result<Self, ApiError> {
        Ok(Self {
            status,
            json: to_json(value)?,
        })
    }
}

/// HTTP API error response: status code and error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// Map a domain [`Refusal`] on review `id` to an [`ApiError`] with the
    /// appropriate HTTP status code.
    pub(crate) fn refused(id: ReviewId, refusal: &Refusal) -> Self {
        let status = match refusal {
            Refusal::Comment(_, Why::Missing | Why::Withdrawn) | Refusal::Approval(_) => {
                StatusCode::NOT_FOUND
            }
            Refusal::EmptyBody
            | Refusal::SuggestionWithoutRange
            | Refusal::ApprovalWithoutRange
            | Refusal::NothingToChange
            | Refusal::Comment(_, Why::VerdictRequired | Why::VerdictRefused) => {
                StatusCode::BAD_REQUEST
            }
            Refusal::Closed
            | Refusal::Comment(..)
            | Refusal::NothingToSend
            | Refusal::Bound(_)
            | Refusal::PassRunning(_) => StatusCode::CONFLICT,
        };

        Self::new(status, refusal.message(id))
    }

    /// Serialize the error as `{"error": "..."}`.
    pub(crate) fn body(&self) -> String {
        // A struct of one string always serializes.
        serde_json::to_string(&ErrorBody {
            error: self.message.clone(),
        })
        .unwrap_or_default()
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl From<Failure> for ApiError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Refused(id, refusal) => Self::refused(id, &refusal),
            Failure::Other(message) => message.into(),
        }
    }
}

/// The pass `request` asks for on the review `id`.
///
/// A pass submitted over HTTP runs under the configured
/// `plugins.command.review.options.cfg` alone: a request naming configurations
/// would run whatever any caller reaching the port asked for.
pub(crate) fn pass_request(id: ReviewId, request: PassRequest) -> Result<Request, ApiError> {
    let conversation = request
        .conversation
        .map(|conversation| conversation.parse::<Conversation>())
        .transpose()
        .map_err(ApiError::invalid)?;

    Ok(Request::new(
        id,
        request.only,
        request.message,
        conversation,
        vec![],
    ))
}

/// The review a request path names: `R-1h4k2m9`, `R1h4k2m9`, or the bare body.
pub(crate) fn review_id(text: &str) -> Result<ReviewId, ApiError> {
    text.parse()
        .map_err(|_| ApiError::not_found(format!("No review {text}.")))
}

/// The comment of the review `id` a request path names by its number.
pub(crate) fn comment_ref(id: ReviewId, number: &str) -> Result<CommentRef, ApiError> {
    number
        .parse()
        .ok()
        .filter(|number| *number > 0)
        .map(|number| CommentRef { review: id, number })
        .ok_or_else(|| ApiError::not_found(format!("No comment {id}#{number}.")))
}

/// The approval of the review `id` a request path names by its number.
pub(crate) fn approval_number(id: ReviewId, number: &str) -> Result<u32, ApiError> {
    number
        .parse()
        .map_err(|_| ApiError::not_found(format!("{id} has no approval {number}.")))
}

/// A version a request path names by its number.
pub(crate) fn version_number(id: ReviewId, number: &str) -> Result<u32, ApiError> {
    number
        .parse()
        .map_err(|_| ApiError::not_found(format!("{id} has no version {number}.")))
}

/// Refuse with a 404 unless the store holds the review `id`.
///
/// A review is never deleted, so one that exists now still does when the
/// operation reads it.
pub(crate) fn exists(store: &Store, id: ReviewId) -> Result<(), ApiError> {
    if store.exists(id) {
        return Ok(());
    }

    Err(ApiError::not_found(format!("No review {id}.")))
}

/// The review `id`, with any pass left by a process that is gone recorded as
/// interrupted.
fn read(store: &Store, id: ReviewId) -> Result<Review, ApiError> {
    exists(store, id)?;

    Ok(store.read(id)?)
}

/// The version `asked` names, or the latest; a 404 when the review has none.
fn version(id: ReviewId, review: &Review, asked: Option<u32>) -> Result<u32, ApiError> {
    cmd::version(id, review, asked).map_err(ApiError::not_found)
}

/// The file among the review's that `path` names exactly; a 404 for any other.
fn tracked<'a>(id: ReviewId, review: &'a Review, path: &str) -> Result<&'a str, ApiError> {
    review
        .paths
        .iter()
        .find(|tracked| *tracked == path)
        .map(String::as_str)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "`{path}` is not a file in {id}, which tracks {}.",
                review.paths.join(", ")
            ))
        })
}

/// `GET /api/reviews`: every review, oldest first, and every one that can't be
/// read with the reason.
pub(crate) fn list(workspace: &Workspace) -> Result<Reply, ApiError> {
    let mut reviews = vec![];
    let mut unreadable = vec![];
    for entry in workspace.store.list()? {
        match entry.review {
            Ok(review) => reviews.push(view::summary(entry.id, &review)),
            Err(error) => unreadable.push(Unreadable {
                id: entry.id.to_string(),
                error,
            }),
        }
    }

    Reply::new(StatusCode::OK, &Listing {
        workspace: workspace.name().to_owned(),
        reviews,
        unreadable,
    })
}

/// `POST /api/reviews`: start a review of the files `request` names.
pub(crate) fn create(ctx: &Context<'_>, request: &NewReview) -> Result<Reply, ApiError> {
    if request.files.is_empty() {
        return Err(ApiError::invalid("A review needs at least one file."));
    }
    let paths = cmd::resolve_files(ctx, &request.files).map_err(ApiError::invalid)?;
    // The base command is the configured one: a request naming a command to
    // run would run whatever any caller reaching the port asked for.
    let (id, review) = cmd::create(
        ctx,
        paths,
        request.title.as_deref(),
        ctx.options.base.as_deref(),
    )?;

    Reply::new(StatusCode::CREATED, &view::summary(id, &review))
}

/// `GET /api/reviews/{id}`: return the running pass, whether the working tree
/// changed since the latest version, and (unless `query.revision` is current)
/// the review with its comments projected into `query.version`.
///
/// `here` is the number of the pass running on this server for `id`, if any.
pub(crate) fn detail(
    workspace: &Workspace,
    id: ReviewId,
    query: &DetailQuery,
    here: Option<u32>,
) -> Result<Reply, ApiError> {
    let store = &workspace.store;
    let review = read(store, id)?;
    let shown = version(id, &review, query.version)?;
    let changed = query.revision.is_none_or(|seen| review.revision > seen);
    let texts = if changed {
        load(&store.blobs(id), view::blobs_needed(&review, shown))?
    } else {
        Texts::default()
    };
    let running = review.passes.running();
    // A running pass records the files when it ends, and a file that can't be
    // read can't be what a version holds.
    let working_tree_changed =
        running.is_none() && snapshot::changed(&workspace.root, &review).unwrap_or(true);

    Reply::new(StatusCode::OK, &DetailReply {
        workspace: workspace.name().to_owned(),
        revision: review.revision,
        running: running.map(|pass| Running {
            number: pass.number,
            conversation: pass.conversation.clone(),
            stoppable: here == Some(pass.number),
        }),
        working_tree_changed,
        review: changed.then(|| view::detail(id, &review, shown, &texts)),
    })
}

/// `GET /api/reviews/{id}/versions/{n}/file`: the file `query.path` as version
/// `number` holds it.
pub(crate) fn file(
    store: &Store,
    id: ReviewId,
    number: u32,
    query: &FileQuery,
) -> Result<Reply, ApiError> {
    let review = read(store, id)?;
    let number = version(id, &review, Some(number))?;
    let path = tracked(id, &review, &query.path)?;
    let name = review
        .blob(number, path)
        .ok_or_else(|| ApiError::not_found(format!("{path} is absent from version {number}.")))?;
    let source = store.blobs(id).get(name)?;
    let (html, headings) = match query.format {
        Format::Html => {
            let (html, headings) = rendered(path, &source);
            (Some(html), headings)
        }
        Format::Text => (None, vec![]),
    };

    Reply::new(StatusCode::OK, &FileReply {
        review: id.to_string(),
        version: number,
        path: path.to_owned(),
        html,
        headings,
        source,
    })
}

/// `source`, the file at `path`, as HTML, and its headings: Markdown when its
/// extension says it is, and preformatted text, which has no headings,
/// otherwise.
///
/// A Markdown heading carries the anchor the site gives it as its `id`, so a
/// `#fragment` link written for the site lands on the same heading here.
fn rendered(path: &str, source: &str) -> (String, Vec<Heading>) {
    let markdown = Utf8Path::new(path).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    });
    if !markdown {
        return (md_html::render_text(source).html, vec![]);
    }

    let found = headings(source);
    let anchors = found
        .iter()
        .map(|heading| (heading.line, heading.slug.clone()))
        .collect();
    let listed = found
        .into_iter()
        .map(|heading| Heading {
            level: heading.level,
            text: heading.text,
            anchor: heading.slug,
        })
        .collect();

    (md_html::render_with_anchors(source, &anchors).html, listed)
}

/// `GET /api/reviews/{id}/diff`: what changed from one version to another, in
/// every file or the one `query.path` names.
pub(crate) fn diff(store: &Store, id: ReviewId, query: &DiffQuery) -> Result<Reply, ApiError> {
    let review = read(store, id)?;
    let [from, to] = span(id, &review, query)?;
    let paths = match &query.path {
        Some(path) => vec![tracked(id, &review, path)?.to_owned()],
        None => review.paths.clone(),
    };

    Reply::new(
        StatusCode::OK,
        &cmd::changes(&store.blobs(id), id, &review, &paths, [from, to])?,
    )
}

/// The two versions the diff `query` compares: those of the pass it names, or
/// `from` and `to`, which default to the one before the latest and the latest.
fn span(id: ReviewId, review: &Review, query: &DiffQuery) -> Result<[u32; 2], ApiError> {
    let Some(pass) = query.pass else {
        let to = version(id, review, query.to)?;
        let from = version(
            id,
            review,
            Some(query.from.unwrap_or_else(|| cmd::before(to))),
        )?;
        return Ok([from, to]);
    };

    if query.from.is_some() || query.to.is_some() {
        return Err(ApiError::invalid(
            "Name either `pass`, or `from` and `to`, not both.",
        ));
    }
    match review.passes.span(PassRef::Number(pass)) {
        Ok((_, span)) => Ok(span),
        Err(why @ (Unspanned::NoPasses | Unspanned::Missing(_))) => {
            Err(ApiError::not_found(why.message(id)))
        }
        Err(why) => Err(ApiError::conflict(why.message(id))),
    }
}

/// `POST /api/reviews/{id}/comments`: add a draft comment, or a suggestion.
pub(crate) fn comment(
    ctx: &Context<'_>,
    id: ReviewId,
    request: NewCommentRequest,
) -> Result<Reply, ApiError> {
    exists(&ctx.store, id)?;
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();

    let number = ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        let anchor = anchor(id, review, blobs, request.anchor.unwrap_or_default())?;
        review
            .add_comment(NewComment {
                role: Role::Reviewer,
                author,
                created_at: at,
                content: Content {
                    anchor,
                    body: request.body.trim().to_owned(),
                    suggestion: request.suggestion,
                },
            })
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Ok(Reply {
        status: StatusCode::CREATED,
        json: comments::record(&ctx.store, CommentRef { review: id, number })?,
    })
}

/// `PATCH /api/reviews/{id}/comments/{n}`: change a draft comment, or the body
/// of the draft reply on a sent one.
pub(crate) fn edit(
    ctx: &Context<'_>,
    comment: CommentRef,
    request: EditRequest,
) -> Result<Reply, ApiError> {
    let id = comment.review;
    exists(&ctx.store, id)?;

    ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        let anchor = request
            .anchor
            .map(|anchor| self::anchor(id, review, blobs, anchor))
            .transpose()?;
        let edit = Edit {
            anchor,
            suggestion: request.suggestion,
            body: request.body.map(|body| body.trim().to_owned()),
            expects: request.draft,
        };
        review
            .edit(comment.number, edit)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Ok(Reply {
        status: StatusCode::OK,
        json: comments::record(&ctx.store, comment)?,
    })
}

/// `DELETE /api/reviews/{id}/comments/{n}`: withdraw a draft comment, or the
/// draft reply on a sent one.
pub(crate) fn withdraw(store: &Store, comment: CommentRef) -> Result<Reply, ApiError> {
    let id = comment.review;
    exists(store, id)?;
    let withdrawn = store.update(id, |review, _| {
        review
            .withdraw(comment.number)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Reply::new(StatusCode::OK, &Withdrawal {
        id: comment.to_string(),
        withdrawn,
    })
}

/// `POST /api/reviews/{id}/comments/{n}/replies`: write the reviewer's draft
/// reply on an open comment, replacing any it holds.
pub(crate) fn reply(
    ctx: &Context<'_>,
    comment: CommentRef,
    request: &ReplyRequest,
) -> Result<Reply, ApiError> {
    let id = comment.review;
    exists(&ctx.store, id)?;
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();

    ctx.store.update(id, |review, _| {
        review
            .reply(comment.number, author, at, request.body.trim().to_owned())
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Ok(Reply {
        status: StatusCode::OK,
        json: comments::record(&ctx.store, comment)?,
    })
}

/// `POST /api/reviews/{id}/comments/{n}/resolve`: mark an open comment
/// resolved.
pub(crate) fn resolve(store: &Store, comment: CommentRef) -> Result<Reply, ApiError> {
    let id = comment.review;
    exists(store, id)?;
    let at = now();
    let outcome = store.update(id, |review, _| {
        review
            .resolve(comment.number, at)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Reply::new(StatusCode::OK, &Resolved {
        id: comment.to_string(),
        outcome,
    })
}

/// `POST /api/reviews/{id}/comments/{n}/reopen`: open a resolved comment again.
pub(crate) fn reopen_comment(store: &Store, comment: CommentRef) -> Result<Reply, ApiError> {
    let id = comment.review;
    exists(store, id)?;
    store.update(id, |review, _| {
        comments::reopen_in(review, comment.number)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Ok(Reply {
        status: StatusCode::OK,
        json: comments::record(store, comment)?,
    })
}

/// `POST /api/reviews/{id}/versions`: record what the working tree holds of the
/// review's files as a new version, when it differs from the latest.
///
/// Refused while a pass runs, in this process or another: the version recorded
/// when the pass ends is the one holding the files.
/// A pass left without an outcome by a process that is gone is recorded as
/// interrupted first.
pub(crate) fn record(ctx: &Context<'_>, id: ReviewId) -> Result<Reply, ApiError> {
    exists(&ctx.store, id)?;
    let _pass = ctx
        .store
        .lock_pass(id)?
        .ok_or_else(|| ApiError::conflict(submit::held(id)))?;
    let at = now();

    let (version, recorded) = ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        // Holding `.pass` keeps `update` from settling such a pass itself.
        review.passes.interrupt(at);
        let latest = review.latest().map(|latest| latest.number);
        let version = snapshot::record(&ctx.root, review, blobs, at).map_err(|error| {
            ApiError::conflict(format!("The working tree can't be recorded: {error}"))
        })?;

        Ok::<_, ApiError>((version, latest != Some(version)))
    })?;

    let status = if recorded {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Reply::new(status, &Recorded {
        review: id.to_string(),
        version,
        recorded,
    })
}

/// Why `POST /api/reviews/{id}/stop` stops nothing, when the server runs no
/// pass of the review `id`: another process runs one, or none runs.
pub(crate) fn unstoppable(store: &Store, id: ReviewId) -> Result<Reply, ApiError> {
    let review = read(store, id)?;

    Err(ApiError::conflict(match review.passes.running() {
        Some(pass) => format!(
            "Pass {} of {id} runs in another process; stop it where it runs.",
            pass.number
        ),
        None => format!("No pass of {id} is running."),
    }))
}

/// `POST /api/reviews/{id}/close`: close a review with no pass running.
pub(crate) fn close(store: &Store, id: ReviewId) -> Result<Reply, ApiError> {
    exists(store, id)?;
    let at = now();
    store.update(id, |review, _| {
        review
            .close(at)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Reply::new(StatusCode::OK, &view::summary(id, &store.read(id)?))
}

/// `POST /api/reviews/{id}/reopen`: open a closed review again.
pub(crate) fn reopen(store: &Store, id: ReviewId) -> Result<Reply, ApiError> {
    exists(store, id)?;
    store.update(id, |review, _| Ok::<_, ApiError>(review.reopen()))?;

    Reply::new(StatusCode::OK, &view::summary(id, &store.read(id)?))
}

/// `POST /api/reviews/{id}/approvals`: approve the range `request` names.
pub(crate) fn approve(
    ctx: &Context<'_>,
    id: ReviewId,
    request: AnchorRequest,
) -> Result<Reply, ApiError> {
    exists(&ctx.store, id)?;
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();

    let number = ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        let anchor = anchor(id, review, blobs, request)?;
        review
            .progress
            .approve(anchor, author, at)
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;
    let (approval, _) = reading::approval(&ctx.store, id, number)?;

    Reply::new(StatusCode::CREATED, &approval)
}

/// `DELETE /api/reviews/{id}/approvals/{n}`: take back an approval.
pub(crate) fn unapprove(store: &Store, id: ReviewId, number: u32) -> Result<Reply, ApiError> {
    exists(store, id)?;
    store.update(id, |review, _| {
        review
            .writable()
            .and_then(|()| review.progress.unapprove(number))
            .map_err(|refusal| ApiError::refused(id, &refusal))
    })?;

    Reply::new(StatusCode::OK, &Unapproved {
        review: id.to_string(),
        approval: number,
    })
}

/// `PUT /api/reviews/{id}/read`: record that the file `request` names was read
/// through the last line of its range.
pub(crate) fn read_until(
    ctx: &Context<'_>,
    id: ReviewId,
    request: AnchorRequest,
) -> Result<Reply, ApiError> {
    exists(&ctx.store, id)?;
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();

    let path = ctx.store.update(id, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        let anchor = anchor(id, review, blobs, request)?;
        let Target::Range(path, range) = anchor.target else {
            return Err(ApiError::invalid(
                "Name the last line read, as a range in `path`.",
            ));
        };
        review.progress.read(path.clone(), ReadUntil {
            version: anchor.version,
            line: range.last_line(),
            author,
            created_at: at,
        });

        Ok(path)
    })?;
    let (read, _) = reading::read(&ctx.store, id, &path)?;

    Reply::new(StatusCode::OK, &read)
}

/// `DELETE /api/reviews/{id}/read`: forget how far the file `query.path` was
/// read.
pub(crate) fn unread(store: &Store, id: ReviewId, query: &ReadQuery) -> Result<Reply, ApiError> {
    exists(store, id)?;
    let (path, cleared) = store.update(id, |review, _| {
        let path = tracked(id, review, &query.path)?.to_owned();
        review
            .writable()
            .map_err(|refusal| ApiError::refused(id, &refusal))?;
        let cleared = review.progress.unread(&path);

        Ok::<_, ApiError>((path, cleared))
    })?;

    Reply::new(StatusCode::OK, &Unread {
        review: id.to_string(),
        path,
        cleared,
    })
}

/// What `request` anchors a comment to in `review`.
///
/// A range is read in the version the request names, which is the one the
/// caller read it in; the working tree plays no part.
fn anchor(
    id: ReviewId,
    review: &Review,
    blobs: &Blobs,
    request: AnchorRequest,
) -> Result<Anchor, ApiError> {
    let ranged = request.range.is_some() || request.bytes.is_some();
    if ranged && request.path.is_none() {
        return Err(ApiError::invalid(
            "A range needs `path`, the file it is in.",
        ));
    }
    if ranged && request.version.is_none() {
        return Err(ApiError::invalid(
            "A range needs `version`, the version of the file it was read in.",
        ));
    }

    let version = version(id, review, request.version)?;
    let Some(path) = request.path else {
        return Ok(Anchor {
            version,
            target: Target::Review,
        });
    };
    let path = tracked(id, review, &path)?.to_owned();
    let name = review
        .blob(version, &path)
        .ok_or_else(|| ApiError::invalid(format!("{path} is absent from version {version}.")))?;
    if !ranged {
        return Ok(Anchor {
            version,
            target: Target::File(path),
        });
    }

    let contents = blobs.get(name)?;
    let place = format!("{path} as version {version} holds it");
    let range = range_in(
        &Text::new(&contents),
        request.range.as_deref(),
        request.bytes,
        &place,
    )?;

    Ok(Anchor {
        version,
        target: Target::Range(path, range),
    })
}

/// The range `range` or `bytes` names in `text`, which is `place`: the file as
/// one version holds it.
fn range_in(
    text: &Text<'_>,
    range: Option<&str>,
    bytes: Option<[usize; 2]>,
    place: &str,
) -> Result<Range, ApiError> {
    match (range, bytes) {
        (Some(range), None) => {
            let parsed: Range = range.parse().map_err(ApiError::invalid)?;
            text.bytes(parsed)
                .map(|_| parsed)
                .ok_or_else(|| ApiError::invalid(format!("{range} lies outside {place}.")))
        }
        (None, Some([start, end])) => text.span_of(start..end).ok_or_else(|| {
            ApiError::invalid(format!(
                "Bytes {start} to {end} lie outside {place}, inside a character, or end before \
                 they start."
            ))
        }),
        (Some(_), Some(_)) => Err(ApiError::invalid(
            "Give the range as `range` or as `bytes`, not both.",
        )),
        (None, None) => Err(ApiError::invalid("No range is given.")),
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
