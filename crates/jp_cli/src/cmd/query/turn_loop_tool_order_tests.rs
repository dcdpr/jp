//! How a turn's tool calls share the terminal.
//!
//! These pin what the user sees across several calls in one response: which
//! call is announced when, which prompts open in which order, and what a
//! decision remembered on one call does to the next.
//! Each test records a transcript: the chrome the printer wrote, with a marker
//! where each prompt opened.

use std::{
    collections::VecDeque,
    io::Write,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use camino::Utf8PathBuf;
use inquire::InquireError;
use jp_config::{AppConfig, Config as _, conversation::tool::PartialToolConfig};
use jp_conversation::event::InquiryId;
use jp_inquire::{InlineOption, ReplyEditMode, ReplyOutcome, prompt::PromptBackend};
use jp_printer::{Printer, SharedBuffer};

use super::*;
use crate::signals::testing::TestSignals;

/// A prompt backend that records where each prompt opened in the chrome, and
/// answers inline selects from a script.
struct TranscriptPromptBackend {
    /// Flushed before each prompt is recorded, so the transcript holds what was
    /// on screen when it opened.
    printer: Arc<Printer>,

    /// The printer's chrome (stderr) buffer.
    chrome: SharedBuffer,

    /// The printer's content (stdout) buffer.
    out: SharedBuffer,

    /// How much of the chrome the transcript already holds.
    seen: Mutex<usize>,

    /// The chrome, with a `[prompt: …]` line where each prompt opened.
    transcript: Mutex<String>,

    /// Answers for inline selects, in order.
    answers: Mutex<VecDeque<char>>,

    /// A file the first inline select waits for before it answers.
    ///
    /// Something that happens while the first prompt is open can only be what
    /// creates it.
    wait_for: Option<Utf8PathBuf>,

    /// What the first inline select saw once it stopped waiting: whether the
    /// awaited file appeared, and the content (stdout) written by then.
    seen_while_open: Mutex<Option<(bool, String)>>,

    /// Told when the first inline select opens.
    opened: Arc<Notify>,
}

impl TranscriptPromptBackend {
    /// On the first prompt, tell whoever waits for it, then wait up to five
    /// seconds for the awaited file.
    fn wait(&self) {
        let mut seen = self.seen_while_open.lock().unwrap();
        if seen.is_some() {
            return;
        }
        self.opened.notify_one();
        let appeared = self.wait_for.as_ref().is_none_or(|path| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !path.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            path.exists()
        });
        self.printer.flush();
        *seen = Some((appeared, strip_ansi(&self.out.lock())));
    }

    /// Bring the transcript up to date with the chrome.
    fn catch_up(&self) {
        self.printer.flush();
        let chrome = self.chrome.lock().clone();
        let mut seen = self.seen.lock().unwrap();
        self.transcript.lock().unwrap().push_str(&chrome[*seen..]);
        *seen = chrome.len();
    }

    fn record(&self, message: &str) {
        self.catch_up();
        let message = strip_ansi(message);
        self.transcript
            .lock()
            .unwrap()
            .push_str(&format!("[prompt: {message}]\n"));
    }

    /// The transcript with everything written after the last prompt, without
    /// ANSI styling.
    fn finish(&self) -> String {
        self.catch_up();
        strip_ansi(&self.transcript.lock().unwrap())
    }
}

impl PromptBackend for TranscriptPromptBackend {
    fn inline_select(
        &self,
        message: &str,
        _options: Vec<InlineOption>,
        _default: Option<char>,
        _writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        self.record(message);
        self.wait();
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(InquireError::OperationCanceled)
    }

    fn inline_reply(
        &self,
        message: &str,
        _initial_text: &str,
        _edit_mode: ReplyEditMode,
        _editor_escape: bool,
        _help: Option<&str>,
        _output: Box<dyn Write + Send>,
    ) -> Result<ReplyOutcome, InquireError> {
        self.record(message);
        Err(InquireError::OperationCanceled)
    }

    fn text(
        &self,
        message: &str,
        _default: Option<&str>,
        _writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.record(message);
        Err(InquireError::OperationCanceled)
    }

    fn select(
        &self,
        message: &str,
        _options: Vec<String>,
        _default: Option<usize>,
        _writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.record(message);
        Err(InquireError::OperationCanceled)
    }

    fn password(&self, message: &str, _writer: &mut dyn Write) -> Result<String, InquireError> {
        self.record(message);
        Err(InquireError::OperationCanceled)
    }
}

fn strip_ansi(text: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(text)).expect("valid utf-8 after stripping ANSI")
}

/// A config where each named tool asks before it runs and shows its arguments
/// on the header line.
fn asking_tools(names: &[&str]) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.style.tool_call.show = true;
    for name in names {
        let partial: PartialToolConfig = serde_json::from_value(json!({
            "source": "local",
            "run": "ask",
            "style": {
                "parameters": "function_call",
                "inline_results": "off",
                "results_file_link": "off",
            },
        }))
        .unwrap();
        config.conversation.tools.insert(
            (*name).to_owned(),
            ToolConfig::from_partial(partial, vec![]).unwrap(),
        );
    }
    config
}

/// An executor source whose named tools ask for permission and then answer with
/// `result`.
fn asking_source(names: &[&str]) -> TestExecutorSource {
    names
        .iter()
        .fold(TestExecutorSource::new(), |source, name| {
            source.with_executor(name, |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "result")
                        .with_arguments(req.arguments.clone())
                        .with_permission_info(PermissionInfo {
                            tool_id: req.id.clone(),
                            tool_name: req.name.clone(),
                            tool_source: ToolSource::Local { tool: None },
                            run_mode: RunMode::Ask,
                            arguments: Value::Object(req.arguments.clone()),
                        }),
                )
            })
        })
}

/// A config for one tool, `tool`, described by its argument formatter.
///
/// The formatter is never run: the executor stands in for the service that runs
/// it.
/// `tool` merges over the defaults, so a test sets `run` and `questions` there.
fn described_tool(tool: &Value) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.style.tool_call.show = true;
    let mut value = json!({
        "source": "local",
        "style": {
            "parameters": "describe",
            "inline_results": "full",
            "results_file_link": "off",
        },
    });
    for (key, field) in tool.as_object().unwrap() {
        value[key] = field.clone();
    }
    let partial: PartialToolConfig = serde_json::from_value(value).unwrap();
    config.conversation.tools.insert(
        "tool".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    config
}

/// How a [`StagedExecutor`] prepares.
#[derive(Clone)]
enum Preparing {
    /// At once.
    Instantly,

    /// Creating this file as it starts.
    Marking(Utf8PathBuf),

    /// Telling this as it starts, then waiting to be cancelled.
    Stalling(Arc<Notify>),

    /// Settling the call with this error, as a formatter that fails does.
    Failing(&'static str),
}

/// A call to `tool`, standing in for the execution service: preparing describes
/// it as `call <n>`, where `n` is its `n` argument, followed by the answer to
/// its formatter's question, if it asks one; running answers `ran`.
struct StagedExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,

    /// Whether the call asks before it runs.
    asks: bool,

    /// The question the formatter asks before it describes the call.
    question: Option<Question>,

    preparing: Preparing,

    /// Whether the formatter has described the call.
    described: AtomicBool,

    /// The answers the formatter was given.
    answers: Mutex<IndexMap<String, Value>>,

    /// How many times the tool itself ran.
    runs: Arc<AtomicUsize>,
}

impl StagedExecutor {
    /// A source whose `tool` calls are staged by `stage`, given each request
    /// and how many requests came before it.
    fn source(
        stage: impl Fn(&ToolCallRequest, usize) -> Self + Send + Sync + 'static,
    ) -> TestExecutorSource {
        let made = AtomicUsize::new(0);
        TestExecutorSource::new().with_executor("tool", move |request| {
            Box::new(stage(&request, made.fetch_add(1, Ordering::SeqCst)))
        })
    }

    fn new(request: &ToolCallRequest, asks: bool, runs: &Arc<AtomicUsize>) -> Self {
        Self {
            tool_id: request.id.clone(),
            tool_name: request.name.clone(),
            arguments: request.arguments.clone(),
            asks,
            question: None,
            preparing: Preparing::Instantly,
            described: AtomicBool::new(false),
            answers: Mutex::new(IndexMap::new()),
            runs: Arc::clone(runs),
        }
    }

    fn preparing(mut self, preparing: Preparing) -> Self {
        self.preparing = preparing;
        self
    }

    fn asking(mut self, question: Question) -> Self {
        self.question = Some(question);
        self
    }
}

#[async_trait]
impl Executor for StagedExecutor {
    async fn prepare(
        &self,
        _render_arguments: bool,
        cancellation: CancellationToken,
    ) -> ExecutorResult {
        match &self.preparing {
            Preparing::Instantly => {}
            Preparing::Marking(path) => std::fs::write(path, "").unwrap(),
            Preparing::Stalling(started) => {
                started.notify_one();
                cancellation.cancelled().await;
                return ExecutorResult::Completed(ToolCallResponse {
                    id: self.tool_id.clone(),
                    result: Err("Tool execution cancelled.".into()),
                });
            }
            Preparing::Failing(error) => {
                return ExecutorResult::Completed(ToolCallResponse {
                    id: self.tool_id.clone(),
                    result: Err((*error).into()),
                });
            }
        }
        if let Some(question) = &self.question {
            return ExecutorResult::NeedsInput {
                question: question.clone(),
                source: InquirySource::tool(&self.tool_name),
                accumulated_answers: IndexMap::new(),
            };
        }
        self.described.store(true, Ordering::SeqCst);
        ExecutorResult::AwaitingAdmission
    }

    fn formatted_arguments(&self) -> Option<String> {
        self.described.load(Ordering::SeqCst).then(|| {
            let n = self.arguments.get("n").unwrap_or(&Value::Null);
            let answers = self.answers.lock().unwrap();
            let answers = answers.iter().map(|(id, answer)| format!(" {id}={answer}"));
            format!("call {n}{}", answers.collect::<String>())
        })
    }

    fn tool_id(&self) -> &str {
        &self.tool_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn arguments(&self) -> Map<String, Value> {
        self.arguments.clone()
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        self.asks.then(|| PermissionInfo {
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_source: ToolSource::Local { tool: None },
            run_mode: RunMode::Ask,
            arguments: Value::Object(self.arguments.clone()),
        })
    }

    fn set_arguments(&self, _args: Value) {}

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        _cancellation_token: CancellationToken,
        _stderr: Option<jp_mcp::server::StderrSink>,
    ) -> ExecutorResult {
        // The formatter's question was answered, so it describes the call.
        if !self.described.load(Ordering::SeqCst) {
            self.answers.lock().unwrap().clone_from(answers);
            self.described.store(true, Ordering::SeqCst);
            return ExecutorResult::AwaitingAdmission;
        }
        self.runs.fetch_add(1, Ordering::SeqCst);
        ExecutorResult::Completed(ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok("ran".into()),
        })
    }
}

/// The events requesting `calls`, each with `{"n": i}` as its arguments.
fn call_events(calls: &[(&str, &str)]) -> Vec<Vec<Event>> {
    calls
        .iter()
        .enumerate()
        .map(|(index, (id, name))| {
            vec![
                Event::tool_call_start(index, (*id).to_owned(), (*name).to_owned()),
                Event::tool_call_args(index, format!(r#"{{"n":{index}}}"#)),
                Event::flush(index),
            ]
        })
        .collect()
}

/// A provider that requests `calls` in one response, then ends the turn with a
/// message.
fn calling_provider(calls: &[(&str, &str)]) -> Arc<dyn Provider> {
    let mut events: Vec<Event> = call_events(calls).into_iter().flatten().collect();
    events.push(Event::Finished(FinishReason::Completed));

    Arc::new(SequentialMockProvider {
        responses: vec![events, final_message_events("Done.")],
        call_index: AtomicUsize::new(0),
        model: inquiry_mock_model(),
    })
}

/// A provider whose first response streams `first`, then holds `rest` back
/// until `gate` opens; every later request ends the turn with a message.
struct GatedProvider {
    first: Vec<Event>,
    rest: Vec<Event>,
    gate: Arc<Notify>,
    requests: AtomicUsize,
}

impl GatedProvider {
    /// A response requesting `calls`, of which every call after the first waits
    /// for `gate`.
    fn calls(calls: &[(&str, &str)], gate: Arc<Notify>) -> Self {
        let mut events = call_events(calls);
        let first = events.remove(0);
        let mut rest: Vec<Event> = events.into_iter().flatten().collect();
        rest.push(Event::Finished(FinishReason::Completed));
        Self {
            first,
            rest,
            gate,
            requests: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for GatedProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = inquiry_mock_model();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![inquiry_mock_model()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        if self.requests.fetch_add(1, Ordering::SeqCst) > 0 {
            let events = final_message_events("Done.");
            return Ok(Box::pin(stream::iter(events.into_iter().map(Ok))));
        }
        let gate = Arc::clone(&self.gate);
        let rest = self.rest.clone();
        let held = stream::once(async move { gate.notified().await })
            .flat_map(move |()| stream::iter(rest.clone().into_iter().map(Ok)));
        let first = stream::iter(self.first.clone().into_iter().map(Ok));
        Ok(Box::pin(first.chain(held)))
    }
}

/// A provider that requests `call` to `tool`, and then, asked anything else,
/// tells `asked` and never answers.
///
/// Stands in for a model asked to answer a tool's question.
struct StallingInquiryProvider {
    call: &'static str,
    asked: Arc<Notify>,
    requests: AtomicUsize,
}

#[async_trait]
impl Provider for StallingInquiryProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = inquiry_mock_model();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![inquiry_mock_model()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            let mut events: Vec<Event> = call_events(&[(self.call, "tool")])
                .into_iter()
                .flatten()
                .collect();
            events.push(Event::Finished(FinishReason::Completed));
            return Ok(Box::pin(stream::iter(events.into_iter().map(Ok))));
        }
        self.asked.notify_one();
        Ok(Box::pin(stream::pending()))
    }
}

/// What one turn left behind.
struct Turn {
    /// How the turn ended.
    result: Result<(), Error>,

    /// The chrome, with a marker where each prompt opened.
    transcript: String,

    /// The content (stdout) the turn wrote.
    out: String,

    /// The tool call responses the conversation recorded, in stream order.
    responses: Vec<ToolCallResponse>,

    /// The inquiry responses the conversation recorded, in stream order.
    inquiries: Vec<InquiryResponse>,

    /// Whether the awaited file appeared while the first prompt was open, and
    /// the content written by then.
    seen_while_open: Option<(bool, String)>,
}

/// One turn to drive through the turn loop.
struct Scenario<'a> {
    config: &'a AppConfig,
    source: TestExecutorSource,
    provider: Arc<dyn Provider>,

    /// Answers for inline selects, in order, whether they open a tool's
    /// approval prompt or the tool interrupt menu.
    answers: &'a [char],

    /// A file the first prompt waits for before it answers.
    wait_for: Option<Utf8PathBuf>,

    /// Told when the first prompt opens.
    opened: Arc<Notify>,

    /// The router the turn's interrupts arrive through.
    router: SignalRouter,
}

impl<'a> Scenario<'a> {
    fn new(
        config: &'a AppConfig,
        source: TestExecutorSource,
        provider: Arc<dyn Provider>,
        answers: &'a [char],
    ) -> Self {
        Self {
            config,
            source,
            provider,
            answers,
            wait_for: None,
            opened: Arc::new(Notify::new()),
            router: detached_router(),
        }
    }

    /// Run the turn, with prompts shown to a user.
    async fn run(self) -> Turn {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
        let mut workspace = Workspace::in_memory(root).with_backend(storage);
        let lock = workspace
            .create_and_lock_conversation(
                Conversation::default(),
                Arc::new(self.config.clone()),
                None,
            )
            .unwrap();

        let model = self
            .provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();
        let (printer, out, chrome) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let prompts = Arc::new(TranscriptPromptBackend {
            printer: Arc::clone(&printer),
            chrome,
            out: Arc::clone(&out),
            seen: Mutex::new(0),
            transcript: Mutex::new(String::new()),
            answers: Mutex::new(self.answers.iter().copied().collect()),
            wait_for: self.wait_for,
            seen_while_open: Mutex::new(None),
            opened: self.opened,
        });
        let definitions = self.source.tool_definitions();

        let result = run_turn_loop(
            self.provider,
            &model,
            self.config,
            &self.router,
            Utf8Path::new("/tmp"),
            InvocationContext::default(),
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &definitions,
            printer.clone(),
            Arc::clone(&prompts) as Arc<dyn PromptBackend>,
            ToolCoordinator::new(
                self.config.conversation.tools.clone(),
                Box::new(self.source),
            )
            .with_interrupt(self.config.interrupt.tool_call.clone()),
            ChatRequest::from("Run them."),
            PendingStreamTrim::default(),
            self.router.turn_interrupt(),
            TurnInterrupts::none(),
        )
        .await;

        let events = lock.events().clone();
        let transcript = prompts.finish();
        Turn {
            result,
            transcript,
            out: strip_ansi(&out.lock()),
            responses: events
                .iter()
                .filter_map(|event| event.event.as_tool_call_response())
                .cloned()
                .collect(),
            inquiries: events
                .iter()
                .filter_map(|event| event.event.as_inquiry_response())
                .cloned()
                .collect(),
            seen_while_open: prompts.seen_while_open.lock().unwrap().take(),
        }
    }
}

/// Run one turn of `calls` against a scripted `source`, answering inline
/// prompts with `answers`.
async fn run_calls(
    config: &AppConfig,
    source: TestExecutorSource,
    calls: &[(&str, &str)],
    answers: &[char],
) -> (String, Vec<ToolCallResponse>) {
    let turn = Scenario::new(config, source, calling_provider(calls), answers)
        .run()
        .await;
    turn.result.unwrap();
    (turn.transcript, turn.responses)
}

/// Press Ctrl-C once `ready` has been told and the tool phase is listening for
/// it.
///
/// A turn registers three handlers before its calls are driven to their
/// responses: the turn's own, the streaming phase's, and the tool phase's.
/// Pressed before the third, the press would reach the streaming phase instead.
async fn press_ctrl_c_in_tool_phase(
    router: SignalRouter,
    signals: TestSignals,
    ready: Arc<Notify>,
) {
    ready.notified().await;
    while router.handlers_registered() < 3 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    signals.interrupt().await;
}

/// Calls are announced and put up for approval in the order the model sent
/// them: the second call's header waits until the first call's prompt has been
/// answered.
#[tokio::test]
async fn approvals_follow_the_order_the_calls_were_sent_in() {
    timeout(Duration::from_secs(10), async {
        let config = asking_tools(&["tool_a", "tool_b"]);
        let (transcript, responses) = run_calls(
            &config,
            asking_source(&["tool_a", "tool_b"]),
            &[("call_a", "tool_a"), ("call_b", "tool_b")],
            &['y', 'y'],
        )
        .await;

        assert_eq!(
            transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool tool_a(n: \
             0)\n[prompt: Run local tool_a tool?]\nCalling tool tool_b(n: 1)\n[prompt: Run local \
             tool_b tool?]\n\n"
        );
        assert_eq!(responses, vec![
            ToolCallResponse {
                id: "call_a".into(),
                result: Ok("result".into()),
            },
            ToolCallResponse {
                id: "call_b".into(),
                result: Ok("result".into()),
            },
        ]);
    })
    .await
    .unwrap();
}

/// A "yes" remembered for the turn on one call runs a later call to the same
/// tool without asking again, and that call is still announced.
#[tokio::test]
async fn a_remembered_yes_runs_a_later_call_to_the_same_tool_without_asking() {
    timeout(Duration::from_secs(10), async {
        let config = asking_tools(&["tool_a"]);
        let (transcript, responses) = run_calls(
            &config,
            asking_source(&["tool_a"]),
            &[("call_1", "tool_a"), ("call_2", "tool_a")],
            &['Y'],
        )
        .await;

        assert_eq!(
            transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool tool_a(n: \
             0)\n[prompt: Run local tool_a tool?]\nCalling tool tool_a(n: 1)\n\n"
        );
        assert_eq!(responses, vec![
            ToolCallResponse {
                id: "call_1".into(),
                result: Ok("result".into()),
            },
            ToolCallResponse {
                id: "call_2".into(),
                result: Ok("result".into()),
            },
        ]);
    })
    .await
    .unwrap();
}

/// A "no" remembered for the turn skips a later call to the same tool, and
/// nothing of that call is shown: it would be announced, never asked about, and
/// never run.
#[tokio::test]
async fn a_remembered_no_skips_a_later_call_to_the_same_tool_without_showing_it() {
    timeout(Duration::from_secs(10), async {
        let config = asking_tools(&["tool_a"]);
        let (transcript, responses) = run_calls(
            &config,
            asking_source(&["tool_a"]),
            &[("call_1", "tool_a"), ("call_2", "tool_a")],
            &['N'],
        )
        .await;

        assert_eq!(
            transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool tool_a(n: \
             0)\n[prompt: Run local tool_a tool?]\n\n"
        );
        assert_eq!(responses, vec![
            ToolCallResponse {
                id: "call_1".into(),
                result: Ok("Tool skipped by user.".into()),
            },
            ToolCallResponse {
                id: "call_2".into(),
                result: Ok("Tool skipped by user (remembered).".into()),
            },
        ]);
    })
    .await
    .unwrap();
}

/// A call settled while it was being prepared, such as by a formatter that
/// failed, is neither announced nor asked about: nothing was shown that the
/// user could judge it by.
/// With nothing on screen, the text after it is not spaced from it either.
#[tokio::test]
async fn a_call_settled_while_preparing_is_not_asked_about() {
    timeout(Duration::from_secs(10), async {
        let config = described_tool(&json!({"run": "ask"}));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            move |request, _| {
                StagedExecutor::new(request, true, &runs).preparing(Preparing::Failing(
                    "Tool 'tool' was not executed because the argument formatter failed: exit 3",
                ))
            }
        });
        let turn = Scenario::new(&config, source, calling_provider(&[("call_1", "tool")]), &[
        ])
        .run()
        .await;

        turn.result.unwrap();
        assert_eq!(
            turn.transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\n"
        );
        assert_eq!(turn.responses, vec![ToolCallResponse {
            id: "call_1".into(),
            result: Err(
                "Tool 'tool' was not executed because the argument formatter failed: exit 3".into()
            ),
        }]);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    })
    .await
    .unwrap();
}

/// A "no" remembered for the turn hides a later call to the same tool entirely,
/// including what its formatter said about it: that call is never asked about
/// and never runs.
#[tokio::test]
async fn a_remembered_no_hides_a_later_calls_description() {
    timeout(Duration::from_secs(10), async {
        let config = described_tool(&json!({"run": "ask"}));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            move |request, _| StagedExecutor::new(request, true, &runs)
        });
        let turn = Scenario::new(
            &config,
            source,
            calling_provider(&[("call_1", "tool"), ("call_2", "tool")]),
            &['N'],
        )
        .run()
        .await;

        turn.result.unwrap();
        assert_eq!(
            turn.transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool \
             tool\n\ncall 0\n[prompt: Run local tool tool?]\n\n"
        );
        assert_eq!(turn.responses, vec![
            ToolCallResponse {
                id: "call_1".into(),
                result: Ok("Tool skipped by user.".into()),
            },
            ToolCallResponse {
                id: "call_2".into(),
                result: Ok("Tool skipped by user (remembered).".into()),
            },
        ]);
        assert_eq!(turn.inquiries, vec![]);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    })
    .await
    .unwrap();
}

/// A call keeps arriving while an earlier call's approval prompt is open: it is
/// prepared then, and shown, and asked about, once the earlier one is answered.
#[tokio::test]
async fn a_later_call_is_prepared_while_an_earlier_one_is_being_approved() {
    timeout(Duration::from_secs(20), async {
        let tmp = tempdir().unwrap();
        let marker = tmp.path().join("prepared-call-2");
        let config = described_tool(&json!({"run": "ask"}));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            let marker = marker.clone();
            move |request, _| {
                let executor = StagedExecutor::new(request, true, &runs);
                if request.id == "call_2" {
                    return executor.preparing(Preparing::Marking(marker.clone()));
                }
                executor
            }
        });
        // The second call is only sent once the first call's prompt is open,
        // so it can only be prepared while that prompt is open.
        let opened = Arc::new(Notify::new());
        let provider = Arc::new(GatedProvider::calls(
            &[("call_1", "tool"), ("call_2", "tool")],
            Arc::clone(&opened),
        ));
        let mut scenario = Scenario::new(&config, source, provider, &['y', 'y']);
        scenario.wait_for = Some(marker);
        scenario.opened = opened;
        let turn = scenario.run().await;

        turn.result.unwrap();
        assert_eq!(
            turn.seen_while_open.map(|(appeared, _)| appeared),
            Some(true),
            "the second call was not prepared while the first prompt was open"
        );
        // The second call is only shown once the first is answered.
        assert_eq!(
            turn.transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool \
             tool\n\ncall 0\n[prompt: Run local tool tool?]\n\nCalling tool tool\n\ncall \
             1\n[prompt: Run local tool tool?]\n\nran\n\nran\n\n"
        );
        assert_eq!(turn.responses, vec![
            ToolCallResponse {
                id: "call_1".into(),
                result: Ok("ran".into()),
            },
            ToolCallResponse {
                id: "call_2".into(),
                result: Ok("ran".into()),
            },
        ]);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    })
    .await
    .unwrap();
}

/// Calls whose formatters both put a question to the assistant have them
/// answered together, and each call is put up for approval as its formatter
/// describes it with its own answer.
#[tokio::test]
async fn formatter_questions_to_the_assistant_are_answered_for_each_call() {
    timeout(Duration::from_secs(20), async {
        let config = described_tool(&json!({
            "run": "ask",
            "questions": {"confirm": {"target": "assistant"}},
        }));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            move |request, _| {
                StagedExecutor::new(request, true, &runs)
                    .asking(Question::boolean("confirm", "Continue?").unwrap())
            }
        });
        let mut calls: Vec<Event> = call_events(&[("call_1", "tool"), ("call_2", "tool")])
            .into_iter()
            .flatten()
            .collect();
        calls.push(Event::Finished(FinishReason::Completed));
        let provider = Arc::new(SequentialMockProvider {
            responses: vec![
                calls,
                unkeyed_structured_events(&json!(true)),
                unkeyed_structured_events(&json!(true)),
                final_message_events("Done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let turn = Scenario::new(&config, source, provider, &['y', 'y'])
            .run()
            .await;

        turn.result.unwrap();
        assert_eq!(
            turn.transcript,
            "\n── jp (anthropic/test) \
             ─────────────────────────────────────────────────────────\n\nCalling tool \
             tool\n\ncall 0 confirm=true\n[prompt: Run local tool tool?]\n\nCalling tool \
             tool\n\ncall 1 confirm=true\n[prompt: Run local tool tool?]\n\nran\n\nran\n\n"
        );
        let mut inquiries = turn.inquiries;
        inquiries.sort_by_key(|inquiry| inquiry.id().as_str().to_owned());
        assert_eq!(inquiries, vec![
            InquiryResponse::answered(InquiryId::new("call_1.confirm.1"), json!(true)),
            InquiryResponse::answered(InquiryId::new("call_2.confirm.1"), json!(true)),
        ]);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    })
    .await
    .unwrap();
}

/// Text the assistant streams while a call's approval prompt is open waits for
/// the prompt to close: it would otherwise land on a terminal the prompt owns.
#[tokio::test]
async fn text_streamed_during_an_approval_prompt_waits_for_it_to_close() {
    timeout(Duration::from_secs(20), async {
        let tmp = tempdir().unwrap();
        let marker = tmp.path().join("prepared-call-2");
        let config = described_tool(&json!({"run": "ask"}));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            let marker = marker.clone();
            move |request, _| {
                let executor = StagedExecutor::new(request, true, &runs);
                if request.id == "call_2" {
                    return executor.preparing(Preparing::Marking(marker.clone()));
                }
                executor
            }
        });
        // While the first prompt is open, the assistant says something, then
        // requests a second call. That call being prepared means the text
        // before it has already been handed to the printer.
        let opened = Arc::new(Notify::new());
        let mut provider = GatedProvider::calls(
            &[("call_1", "tool"), ("call_2", "tool")],
            Arc::clone(&opened),
        );
        provider.rest.splice(0..0, [
            Event::message(2, "Meanwhile, one more.\n\n"),
            Event::flush(2),
        ]);
        let mut scenario = Scenario::new(&config, source, Arc::new(provider), &['y', 'y']);
        scenario.wait_for = Some(marker);
        scenario.opened = opened;
        let turn = scenario.run().await;

        turn.result.unwrap();
        assert_eq!(
            turn.seen_while_open,
            Some((true, String::new())),
            "text reached the terminal while the prompt owned it"
        );
        assert_eq!(turn.out, "Meanwhile, one more.\n\nDone.\n\n");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    })
    .await
    .unwrap();
}

/// A restart chosen while a call is still being prepared stops the preparation
/// and takes the call from the start again.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_while_a_call_is_being_prepared_prepares_it_again() {
    timeout(Duration::from_secs(10), async {
        let config = described_tool(&json!({"run": "unattended"}));
        let runs = Arc::new(AtomicUsize::new(0));
        let stalled = Arc::new(Notify::new());
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            let stalled = Arc::clone(&stalled);
            move |request, made| {
                let executor = StagedExecutor::new(request, false, &runs);
                if made == 0 {
                    return executor.preparing(Preparing::Stalling(Arc::clone(&stalled)));
                }
                executor
            }
        });
        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_1", "tool", "Done.",
        ));
        let (router, signals) = test_router();
        let pressed = tokio::spawn(press_ctrl_c_in_tool_phase(router.clone(), signals, stalled));

        let mut scenario = Scenario::new(&config, source, provider.clone(), &['t']);
        scenario.router = router;
        let turn = scenario.run().await;
        pressed.await.unwrap();

        turn.result.unwrap();
        assert_eq!(turn.responses, vec![ToolCallResponse {
            id: "call_1".into(),
            result: Ok("ran".into()),
        }]);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "the tool ran once");
        assert_eq!(
            provider.call_index.load(Ordering::SeqCst),
            2,
            "the turn continued after the restarted call"
        );
    })
    .await
    .unwrap();
}

/// Ctrl-C while the assistant is answering a formatter's question acts at once:
/// the question is cancelled, the call answers with its cancellation response,
/// and nothing runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_while_the_assistant_answers_a_formatters_question_cancels_it() {
    timeout(Duration::from_secs(10), async {
        let config = described_tool(&json!({
            "run": "unattended",
            "questions": {"confirm": {"target": "assistant"}},
            "cancellation_response": "stopped",
        }));
        let runs = Arc::new(AtomicUsize::new(0));
        let source = StagedExecutor::source({
            let runs = Arc::clone(&runs);
            move |request, _| {
                StagedExecutor::new(request, false, &runs)
                    .asking(Question::boolean("confirm", "Continue?").unwrap())
            }
        });
        let asked = Arc::new(Notify::new());
        let provider = Arc::new(StallingInquiryProvider {
            call: "call_1",
            asked: Arc::clone(&asked),
            requests: AtomicUsize::new(0),
        });
        let (router, signals) = test_router();
        let pressed = tokio::spawn(press_ctrl_c_in_tool_phase(router.clone(), signals, asked));

        // `s`: Stop (cancel & exit).
        let mut scenario = Scenario::new(&config, source, provider.clone(), &['s']);
        scenario.router = router;
        let turn = scenario.run().await;
        pressed.await.unwrap();

        turn.result.unwrap();
        assert_eq!(turn.responses, vec![ToolCallResponse {
            id: "call_1".into(),
            result: Ok("stopped".into()),
        }]);
        assert_eq!(turn.inquiries, vec![InquiryResponse::Cancelled {
            id: InquiryId::new("call_1.confirm.1"),
            reason: CancellationReason::User,
        }]);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert_eq!(
            provider.requests.load(Ordering::SeqCst),
            2,
            "a stopped turn sends no follow-up"
        );
    })
    .await
    .unwrap();
}
