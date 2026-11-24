//! HTTP server for `jp review serve`.
//!
//! Serves the JSON API in [`crate::api`] on a local port and manages background
//! pass turns via [`crate::running::Passes`].
//! `/` and `/reviews/{id}` answer with the page the browser UI starts in, and
//! `/ui/...` with the files it loads ([`crate::ui`]).
//! Rejects cross-origin state-changing requests ([`origin::same_origin`]) and,
//! when bound to loopback, requests with unrecognized `Host` headers
//! ([`origin::answers`]).
//!
//! Polls the host for configuration changes every [`RELOAD_EVERY`] so updated
//! options apply without restarting: `base` and `allowed_hosts` to the next
//! requests, and `cfg`, `assistant`, and `max_chars` to the next passes, which
//! forward them to their turns.
//! `dir`, `bind`, and `port` remain fixed for the lifetime of the server.
//!
//! When the host sends `shutdown` or closes stdin, the server stops accepting
//! requests, sends `stop` interrupts to all active passes, waits for their
//! outcomes to be recorded, and exits.
//!
//! A review store has at most one server: the server holds the store's server
//! lock while it runs, and writes the address it serves at into the store for
//! `jp review open` to find.

use std::{
    fs::{self, File},
    io::{BufRead, Write},
    net::{IpAddr, TcpListener},
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use camino::Utf8Path;
use jp_plugin::{
    client::Client,
    log::{Level, Log, fields},
    message::{ExitMessage, HostToPlugin, InitMessage, PluginToHost, ReadConfigRequest},
    origin, server,
    stdio::{print_to, send_exit},
};
use jp_review_api::{
    DetailQuery, DiffQuery, EditRequest, FileQuery, NewCommentRequest, NewReview, PassRequest,
    ReplyRequest, StopRequest, Stopping,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::{sync::watch, time};

use crate::{
    api::{self, ApiError, Reply, Workspace},
    browser::Browser,
    id::CommentRef,
    options::{Options, Overrides},
    running::{self, Passes, Stop},
    store::{Serving, Store},
    ui::{self, BUNDLE},
};

/// What the server is described as serving, when it starts.
const SERVING: &str = "reviews";

/// What anyone who reaches a server listening beyond loopback can do.
const REACH: &str =
    "read and change every review in this workspace, and start passes that edit workspace files";

/// How often the server reads the configuration again.
const RELOAD_EVERY: Duration = Duration::from_secs(2);

/// How long the server waits for the host to read the configuration again.
const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// What the server's handlers share.
struct Server {
    /// The workspace as the configuration last read says; swapped whole when it
    /// changes, so an operation runs under one configuration throughout.
    workspace: RwLock<Arc<Workspace>>,
    passes: Arc<Passes>,

    /// The connection to the host, which the server logs through.
    client: Client,

    /// What the command line set in place of the options, which a configuration
    /// read again doesn't change.
    overrides: Overrides,

    /// The address the server listens on, which decides whether it checks the
    /// name a request asks for.
    listening: IpAddr,
}

/// Serve the reviews until the host asks the plugin to stop, or goes away.
///
/// The server listens where `overrides` say, then where the options say
/// ([`Options::listen_on`]), and opens the review list in `open` once it
/// listens, when given.
/// The host's messages arrive on `stdin`, and this plugin's, its log included,
/// go out on `stdout`.
pub(crate) fn run(
    init: &InitMessage,
    overrides: &Overrides,
    open: Option<&dyn Browser>,
    stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
) -> Result<(), String> {
    // Held until the plugin exits, which is what tells another `jp review
    // serve` and `jp review open` that this one runs.
    let (workspace, listener, _serving) = match prepare(init, overrides) {
        Ok(prepared) => prepared,
        Err(message) => return send_exit(&mut stdout, 1, Some(&message)),
    };
    let address = listener
        .local_addr()
        .map_err(|error| format!("Failed to read the address listened on: {error}"))?;
    server::announce(&mut stdout, SERVING, address, REACH)?;
    if let Some(browser) = open
        && let Err(error) = browser.open(&format!("{}/", server::url(address)))
    {
        print_to(&mut stdout, &format!("Warning: {error}\n"), "chrome")?;
    }

    let client = Client::new(Arc::new(Mutex::new(Box::new(stdout))));
    let served = client
        .listen(stdin)
        .map_err(|error| error.to_string())
        .and_then(|shutdown| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("jp-review-serve")
                .build()
                .map_err(|error| format!("Failed to start the async runtime: {error}"))?
                .block_on(serve(
                    workspace,
                    overrides.clone(),
                    client.clone(),
                    listener,
                    shutdown,
                ))
        });

    let (code, reason) = match served {
        Ok(()) => (0, None),
        Err(reason) => (1, Some(reason)),
    };
    // A host that is gone can't be told; the plugin exits either way.
    drop(client.send(&PluginToHost::Exit(ExitMessage {
        code,
        reason,
        expected: false,
    })));

    Ok(())
}

/// Read the options, claim the store's server lock, listen where `overrides`
/// and the options say, and write the address listened on into the store.
///
/// Refused while another `jp review serve` serves the store, naming the address
/// it serves at.
fn prepare(
    init: &InitMessage,
    overrides: &Overrides,
) -> Result<(Workspace, TcpListener, File), String> {
    let options = Options::read(&init.options)?.with(overrides);
    let root = init.workspace.root.clone();
    let store = Store::new(options.store_dir(&root, init.paths.user_workspace.as_deref())?);
    let serving = match store.claim_server()? {
        Serving::Claimed(lock) => lock,
        Serving::Running(address) if address.is_empty() => {
            return Err("Another `jp review serve` is starting for these reviews.".to_owned());
        }
        Serving::Running(address) => {
            return Err(format!(
                "`jp review serve` serves these reviews already, at {address}. Open it with `jp \
                 review open`."
            ));
        }
    };
    let listener = server::listen(options.listen_on(&overrides.listen))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("Failed to read the address listened on: {error}"))?;
    store.serving_at(&server::url(address))?;

    let workspace = Workspace {
        root,
        config: init.config.clone(),
        options,
        store,
    };

    Ok((workspace, listener, serving))
}

/// Serve until `shutdown` turns, reading the configuration again as it goes;
/// then stop every pass and wait for each to record how it ended.
async fn serve(
    workspace: Workspace,
    overrides: Overrides,
    client: Client,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let listening = listener
        .local_addr()
        .map_err(|error| format!("Failed to read the address listened on: {error}"))?
        .ip();
    let passes = Arc::new(Passes::new(client.clone()));
    let server = Arc::new(Server {
        workspace: RwLock::new(Arc::new(workspace)),
        passes: Arc::clone(&passes),
        client: client.clone(),
        overrides,
        listening,
    });
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|error| format!("Failed to listen: {error}"))?;
    let following = tokio::spawn(follow_config(client, Arc::clone(&server)));

    let stopping = Arc::clone(&passes);
    let served = axum::serve(listener, router(server))
        .with_graceful_shutdown(async move {
            // `true` once the host sends `shutdown` or closes the connection.
            // An error means nothing can set it any more, which ends the wait
            // too.
            drop(shutdown.wait_for(|stopped| *stopped).await);
            stopping.stop_all();
        })
        .await;
    following.abort();
    passes.wait().await;

    served.map_err(|error| format!("The server failed: {error}"))
}

/// Read the configuration again every [`RELOAD_EVERY`], for the next requests
/// to run under, saying once why a configuration that can't be used is kept
/// out.
async fn follow_config(client: Client, server: Arc<Server>) {
    let mut every = time::interval(RELOAD_EVERY);
    let mut complaint: Option<String> = None;
    // The first tick is now, and the configuration `init` carried is current.
    every.tick().await;

    loop {
        every.tick().await;
        match reload(&client, &server).await {
            Ok(()) => complaint = None,
            Err(error) if complaint.as_ref() != Some(&error) => {
                drop(client.print(
                    &format!("`jp review serve` keeps its settings: {error}\n"),
                    "chrome",
                ));
                complaint = Some(error);
            }
            Err(_) => {}
        }
    }
}

/// Ask the host for the configuration as it loads now, and use it for the next
/// requests when its options can be read.
async fn reload(client: &Client, server: &Server) -> Result<(), String> {
    let request = PluginToHost::ReadConfig(ReadConfigRequest {
        id: Some(client.next_id()),
        path: None,
        reload: true,
    });
    let config = match client.request(&request, Some(RELOAD_TIMEOUT)).await {
        Ok(HostToPlugin::Config(response)) => response.data,
        Ok(HostToPlugin::Error(error)) => return Err(error.message),
        Ok(other) => return Err(format!("the host answered {other:?}")),
        Err(error) => return Err(error.to_string()),
    };
    let options = Options::read(&options_of(&config)?)?.with(&server.overrides);

    server.reconfigure(config, options);
    Ok(())
}

/// The options `config`, a whole configuration, gives this plugin.
fn options_of(config: &Value) -> Result<Map<String, Value>, String> {
    match config.pointer("/plugins/command/review/options") {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(options)) => Ok(options.clone()),
        Some(_) => Err("`plugins.command.review.options` is not a table.".to_owned()),
    }
}

/// Every route the server answers, behind the checks that refuse a request for
/// a name it doesn't answer to, and a write from another site.
fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/reviews/{id}", get(page))
        .route("/ui/{name}", get(asset))
        .route("/api/reviews", get(list).post(create))
        .route("/api/reviews/{id}", get(detail))
        .route("/api/reviews/{id}/versions", post(record))
        .route("/api/reviews/{id}/versions/{number}/file", get(file))
        .route("/api/reviews/{id}/diff", get(diff))
        .route("/api/reviews/{id}/comments", post(comment))
        .route(
            "/api/reviews/{id}/comments/{number}",
            patch(edit).delete(withdraw),
        )
        .route("/api/reviews/{id}/comments/{number}/replies", post(reply))
        .route("/api/reviews/{id}/comments/{number}/resolve", post(resolve))
        .route("/api/reviews/{id}/passes", post(submit))
        .route("/api/reviews/{id}/stop", post(stop))
        .route("/api/reviews/{id}/close", post(close))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&server),
            same_origin_only,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&server),
            known_host_only,
        ))
        .with_state(server)
}

impl Server {
    /// The workspace as the configuration stands now.
    fn workspace(&self) -> Arc<Workspace> {
        Arc::clone(
            &self
                .workspace
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Run the next requests under `config`, a whole configuration, and
    /// `options`, the plugin's read from it.
    ///
    /// The store stays the one the server started with, whatever `dir` says
    /// now, and so does the `dir` a pass forwards to its turn, whose tools have
    /// to find the review in that store.
    fn reconfigure(&self, config: Value, mut options: Options) {
        let mut current = self
            .workspace
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        options.dir.clone_from(&current.options.dir);
        *current = Arc::new(Workspace {
            root: current.root.clone(),
            config,
            options,
            store: current.store.clone(),
        });
    }

    /// Run `operation` against the workspace on a thread that may block.
    async fn work(
        &self,
        operation: impl FnOnce(&Workspace) -> Result<Reply, ApiError> + Send + 'static,
    ) -> Result<Reply, ApiError> {
        let workspace = self.workspace();

        running::blocking(move || operation(&workspace)).await
    }

    /// The response to a request: its reply, or its error, which is logged when
    /// the fault is the server's.
    fn respond(&self, answer: Result<Reply, ApiError>) -> Response {
        match answer {
            Ok(reply) => json_response(reply.status, reply.json),
            Err(failure) => {
                if failure.status.is_server_error() {
                    self.client.log_with(
                        Level::ERROR,
                        "A request failed.",
                        fields([("error", failure.message.clone().into())]),
                    );
                }
                refuse(&failure)
            }
        }
    }
}

/// The response to a request the server doesn't do.
fn refuse(error: &ApiError) -> Response {
    json_response(error.status, error.body())
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// A request body, read as JSON.
///
/// Read whatever its `Content-Type` says: what keeps another site from sending
/// one is the same-origin check, not the header.
fn read_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|error| {
        ApiError::invalid(format!(
            "The request body isn't what this endpoint takes: {error}."
        ))
    })
}

/// A request body that may be left out, read as JSON.
fn optional_body<T: DeserializeOwned + Default>(body: &[u8]) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }

    read_body(body)
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    query
        .map(|Query(query)| query)
        .map_err(|rejection| ApiError::invalid(rejection.body_text()))
}

/// The comment a request path names: its review, and its number there.
fn comment_in(id: &str, number: &str) -> Result<CommentRef, ApiError> {
    api::comment_ref(api::review_id(id)?, number)
}

/// Refuse a write that a page on another site asked for.
async fn same_origin_only(
    State(server): State<Arc<Server>>,
    request: Request,
    next: Next,
) -> Response {
    if !from_another_site(&request) {
        return next.run(request).await;
    }

    server.client.log_with(
        Level::WARN,
        "Refused a write from another site.",
        fields([
            ("method", request.method().as_str().into()),
            ("path", request.uri().path().into()),
        ]),
    );
    refuse(&ApiError::new(
        StatusCode::FORBIDDEN,
        "This request came from another site.",
    ))
}

/// Refuse a request for a name the server doesn't answer to.
///
/// A page on a domain re-pointed at this machine reaches the server as its own
/// origin, so it could read every review and start passes; the name in `Host`
/// is what gives it away.
/// The names allowed are the flags', or the options' as last read.
async fn known_host_only(
    State(server): State<Arc<Server>>,
    request: Request,
    next: Next,
) -> Response {
    // A `Host` that isn't text is no name the server answers to.
    let host = request
        .headers()
        .get(header::HOST)
        .map(|value| value.to_str().unwrap_or_default());
    let workspace = server.workspace();
    let allowed = workspace
        .options
        .listen
        .allowed_hosts(&server.overrides.listen);
    if origin::answers(server.listening, host, allowed) {
        return next.run(request).await;
    }

    let host = host.unwrap_or_default();
    server.client.log_with(
        Level::WARN,
        "Refused a request for a name this server doesn't answer to.",
        fields([("host", host.into()), ("path", request.uri().path().into())]),
    );
    refuse(&ApiError::new(
        StatusCode::FORBIDDEN,
        origin::unknown_host(host, "review"),
    ))
}

/// Whether `request` writes, and a page on another site sent it.
///
/// Reads are left alone: they are as exposed as the port is, which the startup
/// warning says, and a `GET` is where scripts and `curl` live.
fn from_another_site(request: &Request) -> bool {
    let writing = !matches!(*request.method(), Method::GET | Method::HEAD);
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };

    writing && !origin::same_origin(header("sec-fetch-site"), header("origin"), header("host"))
}

async fn not_found(request: Request) -> Response {
    refuse(&ApiError::not_found(format!(
        "No endpoint {} {}.",
        request.method(),
        request.uri().path()
    )))
}

async fn method_not_allowed(request: Request) -> Response {
    refuse(&ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        format!(
            "{} doesn't take {}.",
            request.uri().path(),
            request.method()
        ),
    ))
}

/// The page the browser UI starts in, which routes to the review list or a
/// review itself; the placeholder when the binary carries no UI.
async fn page(State(server): State<Arc<Server>>) -> Response {
    let workspace = server.workspace();
    let html = match &server.overrides.ui {
        Some(dir) => ui::watched_page(workspace.name(), &build_in(dir)),
        None => ui::page(workspace.name(), BUNDLE.as_ref()),
    };

    fresh("text/html; charset=utf-8", html)
}

/// A file the browser UI loads: from the bundle directory `--ui-dir` names, or
/// the one built in.
async fn asset(State(server): State<Arc<Server>>, Path(name): Path<String>) -> Response {
    if let Some(dir) = &server.overrides.ui {
        if name == ui::BUILD {
            return fresh("application/json", Value::String(build_in(dir)).to_string());
        }
        if let Some(content_type) = ui::bundle_file(&name) {
            return match fs::read(dir.join(&name)) {
                Ok(body) => fresh(content_type, body),
                Err(error) => refuse(&ApiError::not_found(format!(
                    "{dir}/{name} can't be read ({error}); `just build-ui review` writes it."
                ))),
            };
        }
    }

    let Some(asset) = ui::asset(&name, BUNDLE.as_ref()) else {
        return refuse(&ApiError::not_found(format!("No file /ui/{name}.")));
    };

    fresh(asset.content_type, asset.body)
}

/// The build the bundle directory `dir` holds; empty while it names none, as
/// while `just build-ui` writes it.
fn build_in(dir: &Utf8Path) -> String {
    fs::read_to_string(dir.join(ui::BUILD)).map_or_default(|build| build.trim().to_owned())
}

/// An answer of `content_type` holding `body`, which the browser asks for again
/// rather than reusing.
fn fresh(content_type: &'static str, body: impl IntoResponse) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn list(State(server): State<Arc<Server>>) -> Response {
    server.respond(server.work(api::list).await)
}

async fn create(State(server): State<Arc<Server>>, body: Bytes) -> Response {
    let answer = async {
        let request: NewReview = read_body(&body)?;
        server
            .work(move |workspace| api::create(&workspace.context(), &request))
            .await
    };

    server.respond(answer.await)
}

async fn detail(
    State(server): State<Arc<Server>>,
    Path(id): Path<String>,
    asked: Result<Query<DetailQuery>, QueryRejection>,
) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let asked = query(asked)?;
        let here = server.passes.running(id);
        server
            .work(move |workspace| api::detail(workspace, id, &asked, here))
            .await
    };

    server.respond(answer.await)
}

async fn file(
    State(server): State<Arc<Server>>,
    Path((id, number)): Path<(String, String)>,
    asked: Result<Query<FileQuery>, QueryRejection>,
) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let number = api::version_number(id, &number)?;
        let asked = query(asked)?;
        server
            .work(move |workspace| api::file(&workspace.store, id, number, &asked))
            .await
    };

    server.respond(answer.await)
}

async fn diff(
    State(server): State<Arc<Server>>,
    Path(id): Path<String>,
    asked: Result<Query<DiffQuery>, QueryRejection>,
) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let asked = query(asked)?;
        server
            .work(move |workspace| api::diff(&workspace.store, id, &asked))
            .await
    };

    server.respond(answer.await)
}

async fn record(State(server): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        server
            .work(move |workspace| api::record(&workspace.context(), id))
            .await
    };

    server.respond(answer.await)
}

async fn comment(
    State(server): State<Arc<Server>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let request: NewCommentRequest = read_body(&body)?;
        server
            .work(move |workspace| api::comment(&workspace.context(), id, request))
            .await
    };

    server.respond(answer.await)
}

async fn edit(
    State(server): State<Arc<Server>>,
    Path((id, number)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let answer = async {
        let comment = comment_in(&id, &number)?;
        let request: EditRequest = read_body(&body)?;
        server
            .work(move |workspace| api::edit(&workspace.context(), comment, request))
            .await
    };

    server.respond(answer.await)
}

async fn withdraw(
    State(server): State<Arc<Server>>,
    Path((id, number)): Path<(String, String)>,
) -> Response {
    let answer = async {
        let comment = comment_in(&id, &number)?;
        server
            .work(move |workspace| api::withdraw(&workspace.store, comment))
            .await
    };

    server.respond(answer.await)
}

async fn reply(
    State(server): State<Arc<Server>>,
    Path((id, number)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let answer = async {
        let comment = comment_in(&id, &number)?;
        let request: ReplyRequest = read_body(&body)?;
        server
            .work(move |workspace| api::reply(&workspace.context(), comment, &request))
            .await
    };

    server.respond(answer.await)
}

async fn resolve(
    State(server): State<Arc<Server>>,
    Path((id, number)): Path<(String, String)>,
) -> Response {
    let answer = async {
        let comment = comment_in(&id, &number)?;
        server
            .work(move |workspace| api::resolve(&workspace.store, comment))
            .await
    };

    server.respond(answer.await)
}

async fn submit(
    State(server): State<Arc<Server>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let request: PassRequest = optional_body(&body)?;
        server
            .passes
            .start(server.workspace(), api::pass_request(id, request)?)
            .await
    };

    server.respond(answer.await)
}

async fn stop(State(server): State<Arc<Server>>, Path(id): Path<String>, body: Bytes) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        let request: StopRequest = optional_body(&body)?;
        match server.passes.stop(id, request.pass)? {
            Stop::Stopping(number) => Reply::new(StatusCode::ACCEPTED, &Stopping {
                review: id.to_string(),
                number,
                stopping: true,
            }),
            Stop::Other { running, asked } => Err(ApiError::conflict(format!(
                "Pass {running} of {id} is running, not pass {asked}; nothing was stopped."
            ))),
            Stop::None => {
                server
                    .work(move |workspace| api::unstoppable(&workspace.store, id))
                    .await
            }
        }
    };

    server.respond(answer.await)
}

async fn close(State(server): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    let answer = async {
        let id = api::review_id(&id)?;
        server
            .work(move |workspace| api::close(&workspace.store, id))
            .await
    };

    server.respond(answer.await)
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
