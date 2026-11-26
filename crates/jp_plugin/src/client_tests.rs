use std::io::{BufReader, PipeWriter};

use pretty_assertions::assert_eq;

use super::*;
use crate::message::{
    CreatedResponse, DoneResponse, ErrorResponse, InterruptRequest, QueryCompleteResponse,
    QueryOutcome, QueryRequest,
};

#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Written {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    }
}

/// Writer that always fails with `BrokenPipe`.
struct Broken;

impl Write for Broken {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
    }
}

fn writing_to(writer: impl Write + Send + 'static) -> Client {
    Client::new(Arc::new(Mutex::new(Box::new(writer))))
}

struct Fixture {
    client: Client,
    written: Written,
    host: PipeWriter,
    shutdown: watch::Receiver<bool>,
}

fn fixture() -> Fixture {
    let written = Written::default();
    let (reader, host) = io::pipe().unwrap();
    let client = writing_to(written.clone());
    let shutdown = client.listen(BufReader::new(reader)).unwrap();

    Fixture {
        client,
        written,
        host,
        shutdown,
    }
}

fn say(host: &mut PipeWriter, line: &str) {
    writeln!(host, "{line}").unwrap();
}

fn send_host(host: &mut PipeWriter, message: &HostToPlugin) {
    say(host, &serde_json::to_string(message).unwrap());
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    time::timeout(Duration::from_secs(5), future)
        .await
        .expect("nothing within 5 seconds")
}

async fn eventually(condition: impl Fn() -> bool) {
    within(async {
        while !condition() {
            time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
}

fn done(id: &str) -> HostToPlugin {
    HostToPlugin::Done(DoneResponse {
        id: Some(id.to_owned()),
    })
}

fn stop(id: &str) -> PluginToHost {
    PluginToHost::Interrupt(InterruptRequest::stop("jp-c1".to_owned()).with_id(id.to_owned()))
}

fn new_conversation(id: &str) -> PluginToHost {
    PluginToHost::Query(QueryRequest {
        id: Some(id.to_owned()),
        conversation: String::new(),
        content: "Hi".to_owned(),
        new: true,
        title: None,
        cfg: vec![],
        schema: None,
        expires_in: None,
    })
}

fn nothing_waits(client: &Client) -> bool {
    lock(&client.inner.pending).is_empty()
}

#[test]
fn ids_count_up_from_one_across_clones() {
    let client = writing_to(Written::default());

    assert_eq!([client.next_id(), client.next_id()], ["1", "2"]);
    assert_eq!(client.clone().next_id(), "3");
}

#[test]
fn a_message_is_written_as_one_line() {
    let written = Written::default();
    let client = writing_to(written.clone());

    client.send(&stop("4")).unwrap();

    assert_eq!(written.lines(), [
        r#"{"type":"interrupt","id":"4","conversation":"jp-c1","action":"stop"}"#
    ]);
}

/// A log layer writes through the client's writer, so their lines never
/// interleave.
#[test]
fn a_print_and_a_line_through_the_shared_writer_go_to_the_host_whole() {
    let written = Written::default();
    let client = writing_to(written.clone());

    client.print("Serving.\n", "chrome").unwrap();
    writeln!(lock(&client.writer()), "{{\"type\":\"exit\",\"code\":0}}").unwrap();

    assert_eq!(written.lines(), [
        r#"{"type":"print","text":"Serving.\n","channel":"chrome","format":"plain"}"#,
        r#"{"type":"exit","code":0}"#,
    ]);
}

#[tokio::test]
async fn a_request_is_sent_and_answered() {
    let mut f = fixture();
    let request = tokio::spawn({
        let client = f.client.clone();
        async move { client.request(&stop("1"), None).await }
    });

    eventually(|| f.written.lines().len() == 1).await;
    send_host(&mut f.host, &done("1"));

    assert_eq!(within(request).await.unwrap().unwrap(), done("1"));
    assert!(nothing_waits(&f.client));
}

#[tokio::test]
async fn an_error_answer_is_returned_as_the_host_sent_it() {
    let mut f = fixture();
    let [answer] = f.client.send_expecting(&stop("1")).unwrap();
    let refused = HostToPlugin::Error(ErrorResponse {
        id: Some("1".to_owned()),
        request: Some("interrupt".to_owned()),
        message: "no turn is running".to_owned(),
    });

    send_host(&mut f.host, &refused);

    assert_eq!(within(answer.wait(None)).await.unwrap(), refused);
}

/// Both responses are written before the caller awaits either `Answer`,
/// verifying that `send_expecting` registers both channels before sending.
#[tokio::test]
async fn two_answers_to_one_request_reach_their_waiters_in_order() {
    let mut f = fixture();
    let [created, ended] = f.client.send_expecting(&new_conversation("1")).unwrap();

    send_host(
        &mut f.host,
        &HostToPlugin::Created(CreatedResponse {
            id: Some("1".to_owned()),
            conversation: "jp-c7".to_owned(),
        }),
    );
    send_host(
        &mut f.host,
        &HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("1".to_owned()),
            conversation: "jp-c7".to_owned(),
            outcome: QueryOutcome::Stopped,
            data: None,
        }),
    );

    assert!(matches!(
        within(created.wait(None)).await,
        Ok(HostToPlugin::Created(created)) if created.conversation == "jp-c7"
    ));
    assert!(matches!(
        within(ended.wait(None)).await,
        Ok(HostToPlugin::QueryComplete(ended)) if ended.outcome == QueryOutcome::Stopped
    ));
    assert!(
        nothing_waits(&f.client),
        "the request is forgotten once its last waiter is served"
    );
}

#[tokio::test]
async fn a_forgotten_request_fails_every_waiter() {
    let f = fixture();
    let [created, ended] = f.client.send_expecting(&new_conversation("1")).unwrap();

    f.client.forget("1");

    assert!(nothing_waits(&f.client));
    assert!(matches!(created.wait(None).await, Err(Error::Closed)));
    assert!(matches!(ended.wait(None).await, Err(Error::Closed)));
}

#[test]
fn a_request_without_an_id_is_not_sent() {
    let written = Written::default();
    let client = writing_to(written.clone());
    let unmatched = PluginToHost::Interrupt(InterruptRequest::stop("jp-c1".to_owned()));

    assert!(matches!(
        client.send_expecting::<1>(&unmatched),
        Err(Error::NoId)
    ));
    assert_eq!(written.lines(), [] as [String; 0]);
    assert!(nothing_waits(&client));
}

#[test]
fn a_request_that_cannot_be_written_leaves_nothing_waiting() {
    let client = writing_to(Broken);

    let error = client
        .send_expecting::<2>(&new_conversation("1"))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "failed to write to the host: broken pipe"
    );
    assert!(nothing_waits(&client));
}

#[tokio::test(start_paused = true)]
async fn a_request_that_times_out_is_forgotten() {
    let client = writing_to(Written::default());

    let error = client
        .request(&stop("1"), Some(Duration::from_secs(30)))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::TimedOut));
    assert!(nothing_waits(&client));
}

#[tokio::test]
async fn a_closed_connection_fails_every_waiting_request_and_shuts_down() {
    let Fixture {
        client,
        host,
        mut shutdown,
        ..
    } = fixture();
    let [answer] = client.send_expecting(&stop("1")).unwrap();

    drop(host);

    assert!(matches!(
        within(answer.wait(None)).await,
        Err(Error::Closed)
    ));
    within(shutdown.wait_for(|stopped| *stopped)).await.unwrap();
    assert!(nothing_waits(&client));
}

#[tokio::test]
async fn a_shutdown_message_signals_shutdown() {
    let mut f = fixture();

    send_host(&mut f.host, &HostToPlugin::Shutdown);

    within(f.shutdown.wait_for(|stopped| *stopped))
        .await
        .unwrap();
}

#[tokio::test]
async fn what_the_client_cannot_use_is_dropped_and_logged() {
    let mut f = fixture();
    let [answer] = f.client.send_expecting(&stop("1")).unwrap();

    say(&mut f.host, "");
    say(&mut f.host, "not a message");
    say(&mut f.host, r#"{"type":"describe"}"#);
    send_host(&mut f.host, &HostToPlugin::Done(DoneResponse { id: None }));
    send_host(&mut f.host, &done("99"));
    send_host(&mut f.host, &done("1"));

    assert_eq!(within(answer.wait(None)).await.unwrap(), done("1"));
    let logged: Vec<(String, String)> = f
        .written
        .lines()
        .iter()
        .filter_map(|line| match serde_json::from_str(line).unwrap() {
            PluginToHost::Log(log) => Some((log.level, log.message)),
            _ => None,
        })
        .collect();
    assert_eq!(logged, [
        (
            "warn".to_owned(),
            "Received a line from the host that isn't a message.".to_owned()
        ),
        (
            "warn".to_owned(),
            "Received a startup message after startup.".to_owned()
        ),
        (
            "warn".to_owned(),
            "Received an answer that names no request.".to_owned()
        ),
        (
            "warn".to_owned(),
            "Received an answer no request waits for.".to_owned()
        ),
    ]);
}
