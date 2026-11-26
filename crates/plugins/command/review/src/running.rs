//! Background pass execution for `jp review serve`.
//!
//! [`Passes::start`] acquires the review's `.pass` lock, snapshots the working
//! tree and records the pass, sends the `query` request to the host, and spawns
//! a background task to await `query_complete` and record the post-turn
//! snapshot and outcome while holding the `.pass` lock.
//!
//! [`Passes::stop`] sends a `stop` interrupt for a running pass, when it is the
//! one the request named.
//! On server shutdown, [`Passes::stop_all`] stops all active passes and rejects
//! new submissions, and [`Passes::wait`] awaits completion of all in-flight
//! pass tasks.

use std::{
    collections::HashMap,
    fs::File,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::http::StatusCode;
use jp_plugin::{
    client::{self, Answer, Client},
    log::{Level, Log, fields},
    message::{HostToPlugin, InterruptRequest, PluginToHost, QueryRequest},
};
use jp_review_api::Started;
use tokio::{
    sync::oneshot,
    task::{self, JoinSet},
    time,
};

use crate::{
    api::{self, ApiError, Reply, Workspace},
    id::ReviewId,
    submit::{self, Ending, Request},
    view,
};

/// How long starting a pass waits for the host to create the conversation a
/// review's first pass runs in.
///
/// The host creates it before anything that can take long, so this only runs
/// out on a host that has stopped answering; the pass goes on regardless.
const CREATED: Duration = Duration::from_secs(30);

/// Why a pass is refused once the server is shutting down.
const SHUTTING_DOWN: &str = "The server is shutting down, and starts no more passes.";

/// The passes this server runs, by review.
pub(crate) struct Passes {
    client: Client,
    flights: Mutex<Flights>,
    tasks: Mutex<JoinSet<()>>,
}

/// The passes running, and whether the server still starts new ones.
#[derive(Default)]
struct Flights {
    running: HashMap<ReviewId, Flight>,

    /// Set when the server shuts down: a pass is refused from then on, and one
    /// whose conversation the host names after that is stopped at once.
    stopping: bool,
}

/// A pass this server runs.
#[derive(Debug, Clone)]
struct Flight {
    number: u32,

    /// The conversation its turn runs in, once the host has named it.
    conversation: Option<String>,

    /// Whether its turn is to be stopped, as soon as its conversation is known.
    stop: bool,
}

/// A pass recorded, its `.pass` lock held, and its query built.
struct Opened {
    number: u32,
    conversation: Option<String>,
    query: QueryRequest,
    lock: File,
}

/// A pass whose turn the host was asked for, which a task waits on.
struct Asked {
    workspace: Arc<Workspace>,
    id: ReviewId,
    number: u32,

    /// The correlation id of the pass's `query`.
    query: String,

    /// The conversation the host creates, when the pass starts one.
    created: Option<Answer>,
    ended: Answer,

    /// The review's `.pass` lock, held until the pass's outcome is recorded.
    lock: File,
}

/// What asking to stop a review's pass found among the passes this server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The pass with this number is asked to stop.
    Stopping(u32),

    /// Pass `running` runs and the request named pass `asked`, so nothing was
    /// stopped.
    Other { running: u32, asked: u32 },

    /// This server runs no pass of the review.
    None,
}

impl Passes {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            flights: Mutex::default(),
            tasks: Mutex::default(),
        }
    }

    /// The number of the pass this server runs for the review `id`, if it runs
    /// one.
    pub(crate) fn running(&self, id: ReviewId) -> Option<u32> {
        lock(&self.flights)
            .running
            .get(&id)
            .map(|flight| flight.number)
    }

    /// Run the pass `request` asks for under `workspace`'s configuration:
    /// record it, ask the host for its turn, and leave a task waiting for the
    /// turn to end.
    ///
    /// Returns once the turn has started: once the host has created the
    /// conversation for a pass that starts one, and once the host has the
    /// request otherwise.
    /// Refused once the server is shutting down.
    pub(crate) async fn start(
        self: &Arc<Self>,
        workspace: Arc<Workspace>,
        request: Request,
    ) -> Result<Reply, ApiError> {
        if lock(&self.flights).stopping {
            return Err(shutting_down());
        }

        let id = request.id;
        let query = self.client.next_id();
        let opening = Arc::clone(&workspace);
        let opened = blocking(move || open(&opening, &request, query)).await?;
        let number = opened.number;
        if !self.register(id, number, opened.conversation.clone()) {
            // The server began shutting down while the pass was recorded.
            let ending = Ending::failed(SHUTTING_DOWN.to_owned());
            self.land(&workspace, id, number, ending, opened.lock).await;
            return Err(shutting_down());
        }

        let asked = match self.ask(&opened.query) {
            Ok((created, ended)) => Asked {
                workspace: Arc::clone(&workspace),
                id,
                number,
                query: opened.query.id.clone().unwrap_or_default(),
                created,
                ended,
                lock: opened.lock,
            },
            Err(error) => {
                let ending = Ending::lost(error.to_string());
                self.land(&workspace, id, number, ending, opened.lock).await;
                return Err(ApiError::from(format!(
                    "Pass {number} of {id} failed: {error}"
                )));
            }
        };

        let starting = self.spawn(asked);
        if let Ok(Ok(Err(error))) = time::timeout(CREATED, starting).await {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("Pass {number} of {id} failed: {error}"),
            ));
        }

        started(&workspace, id, number).await
    }

    /// Ask the host to stop the turn of the pass this server runs for the
    /// review `id`, when it is pass `asked`, or whichever it is when `asked` is
    /// `None`.
    ///
    /// A pass whose conversation the host hasn't named yet is stopped as soon
    /// as it is.
    pub(crate) fn stop(&self, id: ReviewId, asked: Option<u32>) -> Result<Stop, ApiError> {
        let mut flights = lock(&self.flights);
        let Some(flight) = flights.running.get_mut(&id) else {
            return Ok(Stop::None);
        };
        if let Some(asked) = asked.filter(|asked| *asked != flight.number) {
            return Ok(Stop::Other {
                running: flight.number,
                asked,
            });
        }
        flight.stop = true;
        if let Some(conversation) = &flight.conversation {
            self.interrupt(conversation)
                .map_err(|error| ApiError::from(error.to_string()))?;
        }

        Ok(Stop::Stopping(flight.number))
    }

    /// Stop every pass this server runs, and refuse new ones.
    pub(crate) fn stop_all(&self) {
        let conversations: Vec<String> = {
            let mut flights = lock(&self.flights);
            flights.stopping = true;
            flights
                .running
                .values_mut()
                .filter_map(|flight| {
                    flight.stop = true;
                    flight.conversation.clone()
                })
                .collect()
        };
        if !conversations.is_empty() {
            self.client
                .log(Level::INFO, "Stopping the passes this server runs.");
        }

        for conversation in conversations {
            self.stop_turn(&conversation);
        }
    }

    /// Wait for every pass this server runs to record how it ended.
    pub(crate) async fn wait(&self) {
        let mut tasks = std::mem::take(&mut *lock(&self.tasks));
        while tasks.join_next().await.is_some() {}
    }

    /// Note pass `number` as running, unless the server is shutting down,
    /// returning whether it was.
    fn register(&self, id: ReviewId, number: u32, conversation: Option<String>) -> bool {
        let mut flights = lock(&self.flights);
        if flights.stopping {
            return false;
        }

        flights.running.insert(id, Flight {
            number,
            conversation,
            stop: false,
        });
        true
    }

    /// Send the pass's query, waiting for its answers: the conversation the
    /// host creates, when the pass starts one, and the end of the turn.
    fn ask(&self, query: &QueryRequest) -> Result<(Option<Answer>, Answer), client::Error> {
        let message = PluginToHost::Query(query.clone());
        if query.new {
            let [created, ended] = self.client.send_expecting(&message)?;
            return Ok((Some(created), ended));
        }

        let [ended] = self.client.send_expecting(&message)?;
        Ok((None, ended))
    }

    /// Leave a task waiting for the turn of `asked` to end, returning what it
    /// says once the turn has started, or why it didn't.
    fn spawn(self: &Arc<Self>, asked: Asked) -> oneshot::Receiver<Result<(), String>> {
        let (started, starting) = oneshot::channel();
        let passes = Arc::clone(self);
        let mut tasks = lock(&self.tasks);
        // Reaped as new ones start, so a server that runs for days doesn't keep
        // what every pass's task returned.
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move { passes.fly(asked, started).await });

        starting
    }

    /// Wait for the turn of `asked` to end, recording the conversation the host
    /// creates for it on the way, and record how it ended.
    async fn fly(&self, asked: Asked, started: oneshot::Sender<Result<(), String>>) {
        let Asked {
            workspace,
            id,
            number,
            query,
            created,
            ended,
            lock,
        } = asked;

        let ending = match self.created(&workspace, id, number, &query, created).await {
            Ok(()) => {
                drop(started.send(Ok(())));
                ending(ended.wait(None).await)
            }
            Err(ending) => {
                drop(started.send(Err(ending.error.clone().unwrap_or_default())));
                ending
            }
        };

        self.land(&workspace, id, number, ending, lock).await;
    }

    /// Wait for the conversation the host creates for a pass that starts one,
    /// and record it; the turn's ending when the host refused the pass instead.
    async fn created(
        &self,
        workspace: &Arc<Workspace>,
        id: ReviewId,
        number: u32,
        query: &str,
        created: Option<Answer>,
    ) -> Result<(), Ending> {
        let Some(created) = created else {
            return Ok(());
        };
        let conversation = match created.wait(None).await {
            Ok(HostToPlugin::Created(created)) => created.conversation,
            answer => {
                // A host that refused the query answers once, so the turn's end
                // won't come.
                self.client.forget(query);
                return Err(ending(answer));
            }
        };

        let recording = Arc::clone(workspace);
        let named = conversation.clone();
        let recorded = blocking(move || {
            Ok(recording.store.update(id, |review, _| {
                review.passes.created(number, &named);
                Ok::<_, String>(())
            })?)
        })
        .await;
        if let Err(error) = recorded {
            self.client.log_with(
                Level::WARN,
                "Failed to record the conversation of a pass; the assistant's replies from it are \
                 refused.",
                fields([
                    ("review", id.to_string().into()),
                    ("pass", number.into()),
                    ("error", error.message.into()),
                ]),
            );
        }
        self.named(id, number, &conversation);

        Ok(())
    }

    /// Note the conversation the turn of pass `number` runs in, and stop the
    /// turn when that was asked for before it was known.
    fn named(&self, id: ReviewId, number: u32, conversation: &str) {
        let stop = {
            let mut flights = lock(&self.flights);
            let stopping = flights.stopping;
            let Some(flight) = flights
                .running
                .get_mut(&id)
                .filter(|flight| flight.number == number)
            else {
                return;
            };
            flight.conversation = Some(conversation.to_owned());
            flight.stop || stopping
        };

        if stop {
            self.stop_turn(conversation);
        }
    }

    /// Ask the host to stop the turn running in `conversation`, logging a
    /// request that can't be sent.
    fn stop_turn(&self, conversation: &str) {
        if let Err(error) = self.interrupt(conversation) {
            self.client.log_with(
                Level::WARN,
                "Failed to stop a pass.",
                fields([
                    ("conversation", conversation.into()),
                    ("error", error.to_string().into()),
                ]),
            );
        }
    }

    /// Snapshot the working tree, record the outcome of pass `number`, and
    /// release its `.pass` lock.
    async fn land(
        &self,
        workspace: &Arc<Workspace>,
        id: ReviewId,
        number: u32,
        ending: Ending,
        held: File,
    ) {
        let outcome = ending.outcome;
        let recording = Arc::clone(workspace);
        let recorded =
            blocking(move || Ok(submit::finish(&recording.context(), id, number, ending)?)).await;

        // Release the `.pass` lock only after persisting the outcome so a
        // subsequent pass cannot start before this pass's outcome is recorded.
        lock(&self.flights).running.remove(&id);
        drop(held);

        let mut logged = fields([("review", id.to_string().into()), ("pass", number.into())]);
        match recorded {
            Ok(_) => {
                logged.insert("outcome".to_owned(), outcome.to_string().into());
                self.client.log_with(Level::INFO, "A pass ended.", logged);
            }
            Err(error) => {
                logged.insert("error".to_owned(), error.message.into());
                self.client
                    .log_with(Level::WARN, "Failed to record how a pass ended.", logged);
            }
        }
    }

    fn interrupt(&self, conversation: &str) -> Result<(), client::Error> {
        self.client
            .send(&PluginToHost::Interrupt(InterruptRequest::stop(
                conversation.to_owned(),
            )))
    }
}

/// The refusal of a pass submitted while the server shuts down.
fn shutting_down() -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, SHUTTING_DOWN)
}

/// The pass `number` of the review `id`, as recorded.
async fn started(workspace: &Arc<Workspace>, id: ReviewId, number: u32) -> Result<Reply, ApiError> {
    let reading = Arc::clone(workspace);
    blocking(move || {
        let review = reading.store.peek(id)?;
        let pass = review
            .passes
            .get(number)
            .ok_or_else(|| ApiError::from(format!("{id} has no pass {number}.")))?;

        Reply::new(StatusCode::ACCEPTED, &Started {
            review: id.to_string(),
            pass: view::pass(pass),
        })
    })
    .await
}

/// Take the review's `.pass` lock, record the pass `request` asks for, and
/// build the query, carrying the correlation id `query`, that asks the host for
/// its turn.
///
/// A pass recorded and refused a message records its failure and fails.
fn open(workspace: &Workspace, request: &Request, query: String) -> Result<Opened, ApiError> {
    let ctx = workspace.context();
    let id = request.id;
    api::exists(&ctx.store, id)?;
    let lock = ctx
        .store
        .lock_pass(id)?
        .ok_or_else(|| ApiError::conflict(submit::held(id)))?;
    let (number, review) = submit::start(&ctx, request)?;
    let conversation = review
        .passes
        .get(number)
        .and_then(|pass| pass.conversation.clone());

    let content = match submit::message(&ctx, id, &review, number) {
        Ok(content) => content,
        Err(error) => {
            submit::finish(&ctx, id, number, Ending::failed(error.clone()))?;
            return Err(error.into());
        }
    };

    Ok(Opened {
        number,
        conversation: conversation.clone(),
        query: submit::query(
            &ctx,
            request,
            &review,
            conversation.as_deref(),
            query,
            content,
        ),
        lock,
    })
}

/// How the turn ended, by the host's last answer to its query.
fn ending(answer: Result<HostToPlugin, client::Error>) -> Ending {
    match answer {
        Ok(HostToPlugin::QueryComplete(done)) => Ending::of(done),
        Ok(HostToPlugin::Error(error)) => Ending::failed(error.message),
        Ok(other) => Ending::failed(format!("unexpected answer to a pass's query: {other:?}")),
        Err(error) => Ending::lost(error.to_string()),
    }
}

/// Run `operation`, which reads or writes the store, on a thread that may
/// block.
pub(crate) async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    task::spawn_blocking(operation)
        .await
        .map_err(|error| ApiError::from(format!("The request failed: {error}")))?
}

/// Lock `mutex`, recovering the inner value if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "running_tests.rs"]
mod tests;
