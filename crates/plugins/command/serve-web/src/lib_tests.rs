use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};

use jp_plugin::{
    Manifest,
    log::LogLayer,
    message::{ExitMessage, OutputFormat, PathsInfo, WorkspaceInfo},
};
use pretty_assertions::assert_eq;
use serde_json::{Map, Value, json};

use super::*;

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
    fn messages(&self) -> Vec<PluginToHost> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn answer(message: &HostToPlugin) -> Vec<PluginToHost> {
    let input = format!("{}\n", serde_json::to_string(message).unwrap());
    let written = Written::default();
    let (_, log_handle) = LogLayer::new("jp_serve_web");

    run(
        io::Cursor::new(input.into_bytes()),
        written.clone(),
        &log_handle,
    )
    .unwrap();

    written.messages()
}

fn exit_of(args: &[&str], options: Value) -> ExitMessage {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };
    let init = InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: "/work".into(),
            storage: "/work/.jp".into(),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: None,
    };

    answer(&HostToPlugin::Init(Box::new(init)))
        .into_iter()
        .find_map(|message| match message {
            PluginToHost::Exit(exit) => Some(exit),
            _ => None,
        })
        .expect("the plugin exits")
}

#[test]
fn describe_answers_from_the_embedded_manifest() {
    let messages = answer(&HostToPlugin::Describe);

    let [PluginToHost::Describe(describe)] = messages.as_slice() else {
        panic!("expected a describe answer, got {messages:?}");
    };
    assert_eq!(describe.manifest, Manifest {
        protocol: 9,
        description: "Web UI for browsing conversations and continuing them".to_owned(),
        command: vec!["serve".to_owned(), "web".to_owned()],
    });
    assert_eq!(describe.name, "serve-web");
    assert_eq!(describe.help, HELP_TEXT);
}

/// The host refuses a plugin before spawning it when the manifest asks for a
/// newer protocol, so the manifest has to state the same need the handshake
/// does.
#[test]
fn the_manifest_states_the_protocol_the_handshake_needs() {
    let manifest = jp_plugin::manifest::find(MANIFEST.as_bytes())
        .unwrap()
        .unwrap();

    assert_eq!(manifest.protocol, REQUIRED_PROTOCOL);
}

#[test]
fn the_flags_name_an_address_and_a_port_either_way_they_are_written() {
    let args =
        |args: &[&str]| -> Vec<String> { args.iter().map(|arg| (*arg).to_owned()).collect() };

    assert_eq!(
        parse_args(&args(&["--bind", "localhost", "--port=8080"])),
        Ok(Request::Serve(Listen {
            bind: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: Some(8080),
            allowed_hosts: None,
        }))
    );
    assert_eq!(
        parse_args(&args(&["--bind=0.0.0.0"])),
        Ok(Request::Serve(Listen {
            bind: Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ..Listen::default()
        }))
    );
    assert_eq!(parse_args(&args(&["--port", "1", "-h"])), Ok(Request::Help));
}

/// Each `--allowed-host` adds a name, rather than replacing the one before.
#[test]
fn each_allowed_host_flag_adds_a_name() {
    let args = [
        "--allowed-host",
        "My-Machine.local",
        "--allowed-host=.ts.net",
    ];

    assert_eq!(
        parse_args(&args.map(ToOwned::to_owned)),
        Ok(Request::Serve(Listen {
            allowed_hosts: Some(vec!["my-machine.local".to_owned(), ".ts.net".to_owned()]),
            ..Listen::default()
        }))
    );
}

/// A flag that can't be used is refused, rather than ignored in favor of the
/// default.
#[test]
fn a_flag_that_cant_be_used_is_refused() {
    let refused = |args: &[&str]| {
        parse_args(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>()).unwrap_err()
    };

    assert_eq!(
        refused(&["--port", "http"]),
        "`--port` must be a port, from 0 to 65535, not `http`."
    );
    assert_eq!(
        refused(&["--bind=my-machine"]),
        "`--bind`: `my-machine` is not an IP address, such as `127.0.0.1`, or `localhost`."
    );
    assert_eq!(refused(&["--bind"]), "`--bind` needs a value.");
    assert_eq!(
        refused(&["--allowed-host", "box:3000"]),
        "`--allowed-host`: `box:3000` is not a host name; write the name alone, without a scheme \
         or a port, such as `my-machine.local`."
    );
    assert_eq!(
        refused(&["--verbose"]),
        "`jp serve web` takes `--bind`, `--port`, and `--allowed-host`, not `--verbose`."
    );
}

/// A bad option or flag ends the run with the reason, which the host prints.
#[test]
fn the_server_doesnt_start_on_an_address_it_cant_use() {
    assert_eq!(exit_of(&[], json!({ "port": "8080" })), ExitMessage {
        code: 1,
        reason: Some(
            "`plugins.command.serve-web.options.port` must be a port, from 0 to 65535, not \
             \"8080\"."
                .to_owned()
        ),
        expected: false,
    });
    assert_eq!(
        exit_of(&["--bind", "nowhere"], Map::new().into()).reason,
        Some(
            "`--bind`: `nowhere` is not an IP address, such as `127.0.0.1`, or `localhost`."
                .to_owned()
        )
    );
}

#[test]
fn help_is_printed_instead_of_starting_the_server() {
    assert_eq!(exit_of(&["--help"], json!({})), ExitMessage {
        code: 0,
        reason: None,
        expected: false,
    });
}
