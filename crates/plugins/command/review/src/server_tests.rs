use std::{
    fs,
    io::{self, BufReader},
    net::SocketAddr,
    thread,
};

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{ConfigResponse, ErrorResponse, InterruptAction, QueryOutcome};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::tests::test_support::{CONVERSATION, Host, Written, eventually, http, http_text};

/// What a page of the server's own sends.
const SAME_ORIGIN: [(&str, &str); 1] = [("Sec-Fetch-Site", "same-origin")];

/// A server for a workspace named `notes` holding `docs/a.md`, listening on a
/// loopback port, whose host the test plays.
struct Served {
    tmp: Utf8TempDir,
    address: SocketAddr,
    server: Arc<Server>,
    client: Client,
    written: Written,
    host: Host,
    _shutdown: watch::Receiver<bool>,
}

async fn served() -> Served {
    serve_in(Utf8TempDir::new().unwrap(), Overrides::default()).await
}

/// A server as `served` starts, serving the UI from `ui/` in its temporary
/// directory, as `--ui-dir` asks.
async fn served_from_ui_dir() -> Served {
    let tmp = Utf8TempDir::new().unwrap();
    let overrides = Overrides {
        ui: Some(tmp.path().join("ui")),
        ..Overrides::default()
    };

    serve_in(tmp, overrides).await
}

async fn serve_in(tmp: Utf8TempDir, overrides: Overrides) -> Served {
    let root = tmp.path().join("notes");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.md"), "# Title\n\nFirst line.\n").unwrap();
    let workspace = Workspace {
        store: Store::new(root.join(".user/review")),
        root,
        config: json!({ "user": { "name": "Jane Doe <jane@example.com>" } }),
        options: Options::default(),
    };

    let written = Written::default();
    let (reader, host) = io::pipe().unwrap();
    let client = Client::new(Arc::new(Mutex::new(Box::new(written.clone()))));
    let shutdown = client.listen(BufReader::new(reader)).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = Arc::new(Server {
        workspace: RwLock::new(Arc::new(workspace)),
        passes: Arc::new(Passes::new(client.clone())),
        client: client.clone(),
        overrides,
        listening: address.ip(),
    });
    let serving = Arc::clone(&server);
    tokio::spawn(async move { axum::serve(listener, router(serving)).await });

    Served {
        tmp,
        address,
        server,
        client,
        written,
        host: Host(Some(host)),
        _shutdown: shutdown,
    }
}

impl Served {
    fn get(&self, path: &str) -> (u16, Value) {
        http(self.address, "GET", path, &[], "")
    }

    /// Send `body` with `method`, as a page of the server's own does.
    fn send(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        http(self.address, method, path, &SAME_ORIGIN, body)
    }

    /// Start a review of `docs/a.md`, returning its id.
    fn start(&self) -> String {
        let (status, review) = self.send("POST", "/api/reviews", r#"{"files":["docs/a.md"]}"#);
        assert_eq!(status, 201, "{review}");

        review["id"].as_str().unwrap().to_owned()
    }
}

/// A page on a domain re-pointed at this machine still names that domain, which
/// a server on loopback refuses, reads included, unless it is allowed.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_for_a_name_the_server_does_not_answer_to_is_refused() {
    let served = served().await;
    let read_as = |host: &str| http(served.address, "GET", "/api/reviews", &[("Host", host)], "");

    assert_eq!(
        read_as("evil.example:3001"),
        (
            403,
            json!({
                "error": "This server doesn't answer to `evil.example`. To reach it by that name, \
                          add `evil.example` to `plugins.command.review.options.allowed_hosts`."
            })
        )
    );
    assert_eq!(
        read_as("localhost:3001"),
        (200, json!({ "workspace": "notes", "reviews": [] }))
    );

    // The names allowed follow the configuration as it is read again.
    let mut options = Options::default();
    options.listen.allowed_hosts = Some(vec![".ts.net".to_owned()]);
    served.server.reconfigure(
        json!({ "user": { "name": "Jane Doe <jane@example.com>" } }),
        options,
    );
    assert_eq!(
        read_as("box.tail1234.ts.net:3001"),
        (200, json!({ "workspace": "notes", "reviews": [] }))
    );

    let refusals: Vec<Value> = served
        .written
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            PluginToHost::Log(log)
                if log.message == "Refused a request for a name this server doesn't answer to." =>
            {
                Some(Value::Object(log.fields))
            }
            _ => None,
        })
        .collect();
    assert_eq!(refusals, [
        json!({ "host": "evil.example:3001", "path": "/api/reviews" })
    ]);
}

/// Both pages a browser opens are the one page, naming the workspace, and the
/// manifest a phone installs from is there whether the UI is built in or not.
#[tokio::test(flavor = "multi_thread")]
async fn the_pages_and_the_ui_files_are_served() {
    let served = served().await;
    let get = |path: &str| http_text(served.address, "GET", path, &[], "");

    let (list, list_type, list_page) = get("/");
    let (review, _, review_page) = get("/reviews/R-1h4k2m9");
    let (manifest, manifest_type, _) = get("/ui/manifest.webmanifest");
    let (icon, icon_type, _) = get("/ui/icon.svg");
    let missing = get("/ui/nope.js");

    assert_eq!(
        (list, list_type.as_str()),
        (200, "text/html; charset=utf-8")
    );
    assert_eq!(review, 200);
    assert_eq!(list_page, review_page);
    assert_eq!(
        list_page,
        ui::page("notes", BUNDLE.as_ref()),
        "the page names the workspace"
    );
    assert_eq!(
        (manifest, manifest_type.as_str()),
        (200, "application/manifest+json")
    );
    assert_eq!((icon, icon_type.as_str()), (200, "image/svg+xml"));
    assert_eq!(
        (missing.0, missing.2.as_str()),
        (404, r#"{"error":"No file /ui/nope.js."}"#)
    );
}

/// With `--ui-dir`, the page names the build the directory holds, the bundle's
/// files are read as they are now, and nothing else in the directory is.
#[tokio::test(flavor = "multi_thread")]
async fn the_ui_is_served_from_a_bundle_directory_as_it_is_rebuilt() {
    let served = served_from_ui_dir().await;
    let ui = served.tmp.path().join("ui");
    let get = |path: &str| http_text(served.address, "GET", path, &[], "");

    // Before the first build: no build named, and no files to load.
    assert_eq!(
        get("/ui/build"),
        (200, "application/json".to_owned(), r#""""#.to_owned())
    );
    // The message carries the system's own reason, which varies by platform.
    assert_eq!(get("/ui/style.css").0, 404);

    fs::create_dir_all(&ui).unwrap();
    fs::write(ui.join("style.css"), "body { margin: 0; }\n").unwrap();
    fs::write(ui.join("build"), "1834927105\n").unwrap();
    assert_eq!(get("/").2, ui::watched_page("notes", "1834927105"));
    assert_eq!(
        get("/ui/style.css"),
        (
            200,
            "text/css".to_owned(),
            "body { margin: 0; }\n".to_owned()
        )
    );

    fs::write(ui.join("style.css"), "body { margin: 1rem; }\n").unwrap();
    fs::write(ui.join("build"), "2290315521\n").unwrap();
    assert_eq!(
        get("/ui/build"),
        (
            200,
            "application/json".to_owned(),
            r#""2290315521""#.to_owned()
        )
    );
    assert_eq!(get("/ui/style.css").2, "body { margin: 1rem; }\n");

    fs::write(ui.join("notes.txt"), "not the UI's").unwrap();
    assert_eq!(
        get("/ui/notes.txt"),
        (
            404,
            "application/json".to_owned(),
            r#"{"error":"No file /ui/notes.txt."}"#.to_owned()
        )
    );
    assert_eq!(get("/ui/manifest.webmanifest").0, 200);
}

/// The pages are reads, and the name a request asks for still decides whether
/// the server answers it.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_for_a_name_the_server_does_not_answer_to_is_refused() {
    let served = served().await;

    let (status, _, _) = http_text(served.address, "GET", "/", &[("Host", "evil.example")], "");

    assert_eq!(status, 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_is_started_and_listed() {
    let served = served().await;
    assert_eq!(
        served.get("/api/reviews"),
        (200, json!({ "workspace": "notes", "reviews": [] }))
    );

    let id = served.start();

    let (status, listing) = served.get("/api/reviews");
    assert_eq!(status, 200);
    assert_eq!(listing["reviews"][0]["id"], id);
    assert_eq!(listing["reviews"][0]["files"], json!(["docs/a.md"]));
}

/// A page on another site can make the browser send it a write, but can't read
/// the answer: its writes are refused, and reads are left alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_from_another_site_is_refused() {
    let served = served().await;
    let body = r#"{"files":["docs/a.md"]}"#;
    let own = format!("http://{}", served.address);

    let cross_site = http(
        served.address,
        "POST",
        "/api/reviews",
        &[("Sec-Fetch-Site", "cross-site")],
        body,
    );
    let other_origin = http(
        served.address,
        "POST",
        "/api/reviews",
        &[("Origin", "https://example.test")],
        body,
    );
    let read = http(
        served.address,
        "GET",
        "/api/reviews",
        &[("Sec-Fetch-Site", "cross-site")],
        "",
    );
    let own_origin = http(
        served.address,
        "POST",
        "/api/reviews",
        &[("Origin", own.as_str())],
        body,
    );

    let refused = (
        403,
        json!({ "error": "This request came from another site." }),
    );
    assert_eq!(cross_site, refused);
    assert_eq!(other_origin, refused);
    assert_eq!(read, (200, json!({ "workspace": "notes", "reviews": [] })));
    assert_eq!(own_origin.0, 201);
    let refusals: Vec<Value> = served
        .written
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            PluginToHost::Log(log) if log.message == "Refused a write from another site." => {
                Some(Value::Object(log.fields))
            }
            _ => None,
        })
        .collect();
    assert_eq!(refusals, [
        json!({ "method": "POST", "path": "/api/reviews" }),
        json!({ "method": "POST", "path": "/api/reviews" }),
    ]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_the_server_cant_take_is_answered_as_json() {
    let served = served().await;
    let id = served.start();

    let (malformed, refused) = served.send("POST", "/api/reviews", "{");
    let (bad_query, rejected) = served.get(&format!("/api/reviews/{id}?version=x"));

    assert_eq!(
        served.get("/api/nope"),
        (404, json!({ "error": "No endpoint GET /api/nope." }))
    );
    assert_eq!(
        served.send("DELETE", "/api/reviews", ""),
        (405, json!({ "error": "/api/reviews doesn't take DELETE." }))
    );
    assert_eq!(
        served.get("/api/reviews/R-0000000"),
        (404, json!({ "error": "No review R-0000000." }))
    );
    assert_eq!(malformed, 400);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .starts_with("The request body isn't what this endpoint takes: "),
        "{refused}"
    );
    assert_eq!(bad_query, 400);
    assert!(rejected["error"].is_string(), "{rejected}");
}

/// Every route on a comment reaches it: it is added, edited, refused a reply
/// and a resolution while it is a draft, and withdrawn.
#[tokio::test(flavor = "multi_thread")]
async fn the_comment_routes_reach_their_comment() {
    let served = served().await;
    let id = served.start();
    let comments = format!("/api/reviews/{id}/comments");
    let draft = (
        409,
        json!({ "error": format!("{id}#1 is a draft, not yet sent.") }),
    );

    let (added, made) = served.send(
        "POST",
        &comments,
        r#"{"anchor":{"version":1,"path":"docs/a.md","bytes":[9,14]},"body":"Name it."}"#,
    );
    let (patched, edited) = served.send(
        "PATCH",
        &format!("{comments}/1"),
        r#"{"body":"Rename it."}"#,
    );
    let replied = served.send(
        "POST",
        &format!("{comments}/1/replies"),
        r#"{"body":"Why?"}"#,
    );
    let resolved = served.send("POST", &format!("{comments}/1/resolve"), "");
    let withdrawn = served.send("DELETE", &format!("{comments}/1"), "");
    let (closed, review) = served.send("POST", &format!("/api/reviews/{id}/close"), "");

    assert_eq!((added, made["excerpt"].clone()), (201, json!("First")));
    assert_eq!(
        (patched, edited["body"].clone()),
        (200, json!("Rename it."))
    );
    assert_eq!(replied, draft);
    assert_eq!(resolved, draft);
    assert_eq!(
        withdrawn,
        (
            200,
            json!({ "id": format!("{id}#1"), "withdrawn": "comment" })
        )
    );
    assert_eq!((closed, review["status"].clone()), (200, json!("closed")));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_progress_and_reopen_routes_reach_their_review() {
    let served = served().await;
    let id = served.start();
    let review = format!("/api/reviews/{id}");
    // "First line.", bytes 9 to 20 of `docs/a.md`.
    let line = r#"{"version":1,"path":"docs/a.md","range":"3"}"#;
    let draft = (
        409,
        json!({ "error": format!("{id}#1 is a draft, not yet sent.") }),
    );

    let (approved, approval) = served.send("POST", &format!("{review}/approvals"), line);
    let unapproved = served.send("DELETE", &format!("{review}/approvals/1"), "");
    let unnumbered = served.send("DELETE", &format!("{review}/approvals/x"), "");
    let (read, until) = served.send("PUT", &format!("{review}/read"), line);
    let unread = served.send("DELETE", &format!("{review}/read?path=docs/a.md"), "");
    served.send(
        "POST",
        &format!("{review}/comments"),
        r#"{"body":"Overall."}"#,
    );
    let reopened_draft = served.send("POST", &format!("{review}/comments/1/reopen"), "");
    let stale = served.send("DELETE", &format!("{review}/comments/1?draft=reply"), "");
    served.send("POST", &format!("{review}/close"), "");
    let (reopened, summary) = served.send("POST", &format!("{review}/reopen"), "");

    assert_eq!(
        (
            approved,
            approval["number"].clone(),
            approval["excerpt"].clone()
        ),
        (201, json!(1), json!("First line."))
    );
    assert_eq!(unapproved, (200, json!({ "review": id, "approval": 1 })));
    assert_eq!(
        unnumbered,
        (404, json!({ "error": format!("{id} has no approval x.") }))
    );
    assert_eq!(
        (
            read,
            until["line"].clone(),
            until["projected"]["bytes"].clone()
        ),
        (200, json!(3), json!([9, 20]))
    );
    assert_eq!(
        unread,
        (
            200,
            json!({ "review": id, "path": "docs/a.md", "cleared": true })
        )
    );
    assert_eq!(reopened_draft, draft);
    assert_eq!(stale, draft, "a draft comment holds no draft reply");
    assert_eq!((reopened, summary["status"].clone()), (200, json!("open")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_is_served_rendered_and_a_diff_of_one_file() {
    let served = served().await;
    let id = served.start();

    let (status, file) = served.get(&format!(
        "/api/reviews/{id}/versions/1/file?path=docs/a.md&format=html"
    ));
    let changes = served.get(&format!("/api/reviews/{id}/diff?path=docs/a.md"));

    assert_eq!(status, 200);
    assert_eq!(file["source"], "# Title\n\nFirst line.\n");
    assert!(
        file["html"]
            .as_str()
            .unwrap()
            .starts_with(r#"<h1 id="title"><span data-s="2" data-e="7">Title</span></h1>"#),
        "{file}"
    );
    assert_eq!(
        changes,
        (
            200,
            json!({ "review": id, "from": 1, "to": 1, "files": [] })
        )
    );
}

/// A browser starts a pass, watches it run, and stops it, and the pass records
/// that it was stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_is_started_watched_and_stopped() {
    let mut served = served().await;
    let id = served.start();
    served.send(
        "POST",
        &format!("/api/reviews/{id}/comments"),
        r#"{"body":"Too long."}"#,
    );

    // The request returns once the host has created the conversation, which
    // the test does as the host.
    let address = served.address;
    let passes = format!("/api/reviews/{id}/passes");
    let submitting = thread::spawn(move || http(address, "POST", &passes, &SAME_ORIGIN, ""));
    let query = served.written.first_query().await;
    served.host.created(&query);
    let (submitted, pass) = submitting.join().unwrap();

    let (_, watched) = served.get(&format!("/api/reviews/{id}"));
    let stopping_another = served.send("POST", &format!("/api/reviews/{id}/stop"), r#"{"pass":2}"#);
    let interrupts_before = served.written.interrupts().len();
    let stopping = served.send("POST", &format!("/api/reviews/{id}/stop"), r#"{"pass":1}"#);
    let interrupt = served.written.interrupts().remove(0);
    served.host.completed(&query, QueryOutcome::Stopped);
    let detail = format!("/api/reviews/{id}");
    eventually(move || http(address, "GET", &detail, &[], "").1["running"].is_null()).await;
    let (_, ended) = served.get(&format!("/api/reviews/{id}"));

    assert_eq!(submitted, 202);
    assert_eq!(
        (pass["number"].clone(), pass["conversation"].clone()),
        (json!(1), json!(CONVERSATION))
    );
    assert_eq!(
        watched["running"],
        json!({ "number": 1, "conversation": CONVERSATION, "stoppable": true })
    );
    assert_eq!(
        (stopping_another, interrupts_before),
        (
            (
                409,
                json!({
                    "error": format!("Pass 1 of {id} is running, not pass 2; nothing was stopped.")
                })
            ),
            0
        )
    );
    assert_eq!(
        stopping,
        (202, json!({ "review": id, "number": 1, "stopping": true }))
    );
    assert_eq!(
        (interrupt.conversation.as_str(), interrupt.action),
        (CONVERSATION, InterruptAction::Stop)
    );
    assert_eq!(ended["review"]["passes"][0]["ended"]["outcome"], "stopped");
}

/// A file edited outside the review shows on the next poll, and the banner's
/// request records it.
#[tokio::test(flavor = "multi_thread")]
async fn the_working_tree_is_reported_and_recorded() {
    let served = served().await;
    let id = served.start();
    fs::write(
        served.tmp.path().join("notes/docs/a.md"),
        "# Title\n\nEdited.\n",
    )
    .unwrap();

    let (_, polled) = served.get(&format!("/api/reviews/{id}?revision=1"));
    let recorded = served.send("POST", &format!("/api/reviews/{id}/versions"), "");
    let (_, after) = served.get(&format!("/api/reviews/{id}"));

    assert_eq!(polled["working_tree_changed"], true);
    assert_eq!(
        recorded,
        (201, json!({ "review": id, "version": 2, "recorded": true }))
    );
    assert_eq!(
        (
            after["working_tree_changed"].clone(),
            after["review"]["version"].clone()
        ),
        (json!(false), json!(2))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_without_a_pass_here_has_none_to_stop() {
    let served = served().await;
    let id = served.start();

    assert_eq!(
        served.send("POST", &format!("/api/reviews/{id}/stop"), ""),
        (
            409,
            json!({ "error": format!("No pass of {id} is running.") })
        )
    );
}

/// The configuration the host loads now, as `reload` asks for it: the host,
/// played by the test, answers with `answer` for the request's id.
async fn reloaded(
    served: &mut Served,
    answer: impl FnOnce(Option<String>) -> HostToPlugin,
) -> Result<(), String> {
    let reloading = tokio::spawn({
        let client = served.client.clone();
        let server = Arc::clone(&served.server);
        async move { reload(&client, &server).await }
    });
    let written = served.written.clone();
    let asked = move || {
        written
            .messages()
            .into_iter()
            .find_map(|message| match message {
                PluginToHost::ReadConfig(request) => Some(request),
                _ => None,
            })
    };
    let probe = asked.clone();
    eventually(move || probe().is_some()).await;
    let request = asked().unwrap();
    assert!(request.reload, "the host reads the configuration again");

    served.host.answer(&answer(request.id));
    reloading.await.unwrap()
}

/// The next request runs under what the configuration says now; the store stays
/// where the server started, whatever `dir` says, and so does the `dir` a pass
/// hands its turn, for the tools to find the review there.
#[tokio::test(flavor = "multi_thread")]
async fn a_reloaded_configuration_is_what_the_next_requests_run_under() {
    let mut served = served().await;
    let store = served.server.workspace().store.clone();

    let reloaded = reloaded(&mut served, |id| {
        HostToPlugin::Config(ConfigResponse {
            id,
            path: None,
            data: json!({
                "user": { "name": "Ada <ada@example.com>" },
                "plugins": { "command": { "review": { "options": {
                    "cfg": ["skill/review"],
                    "dir": "elsewhere"
                } } } }
            }),
        })
    })
    .await;

    let workspace = served.server.workspace();
    assert_eq!(reloaded, Ok(()));
    assert_eq!(workspace.options.cfg, ["skill/review"]);
    assert_eq!(workspace.store, store);
    assert_eq!(workspace.options.dir, None);
    assert!(
        !workspace
            .options
            .pass_cfg()
            .iter()
            .any(|entry| entry.contains("elsewhere")),
        "{:?}",
        workspace.options.pass_cfg()
    );
    let (_, review) = served.send("POST", "/api/reviews", r#"{"files":["docs/a.md"]}"#);
    assert_eq!(review["author"], "Ada <ada@example.com>");
}

/// A configuration the server can't use keeps the last one that it could.
#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_that_cant_be_used_keeps_the_last_one() {
    let mut served = served().await;

    let bad_options = reloaded(&mut served, |id| {
        HostToPlugin::Config(ConfigResponse {
            id,
            path: None,
            data: json!({ "plugins": { "command": { "review": { "options": {
                "max_chars": 0
            } } } } }),
        })
    })
    .await;

    assert_eq!(
        bad_options,
        Err(
            "`plugins.command.review.options.max_chars` must allow at least one character."
                .to_owned()
        )
    );
    assert_eq!(served.server.workspace().options, Options::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_the_host_cant_load_keeps_the_last_one() {
    let mut served = served().await;

    let refused = reloaded(&mut served, |id| {
        HostToPlugin::Error(ErrorResponse {
            id,
            request: Some("read_config".to_owned()),
            message: ".jp/config.toml: expected a value".to_owned(),
        })
    })
    .await;

    assert_eq!(refused, Err(".jp/config.toml: expected a value".to_owned()));
    assert_eq!(
        served.server.workspace().config,
        json!({ "user": { "name": "Jane Doe <jane@example.com>" } })
    );
}

#[test]
fn the_options_of_a_configuration_are_the_plugin_s() {
    assert_eq!(options_of(&json!({})), Ok(Map::new()));
    assert_eq!(
        options_of(
            &json!({ "plugins": { "command": { "review": { "options": { "cfg": [] } } } } })
        ),
        Ok(json!({ "cfg": [] }).as_object().unwrap().clone())
    );
    assert_eq!(
        options_of(&json!({ "plugins": { "command": { "review": { "options": 1 } } } })),
        Err("`plugins.command.review.options` is not a table.".to_owned())
    );
}
