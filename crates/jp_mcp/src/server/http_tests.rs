use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_tool::{Outcome, ToolDefinition, ToolDocs};
use rmcp::model::CallToolRequestParams;
use serde_json::{Map, Value, json};
use tokio::time::{Duration, timeout};

use super::*;
use crate::{
    Client, Content,
    server::{
        InvocationContext,
        builtin::{BuiltinExecutors, BuiltinTool},
        service::{
            Admission, CallRequest, ConfiguredTool, HostReceiver, Interaction, ReleaseDecision,
            ServiceError,
        },
    },
};

struct Count(Arc<AtomicUsize>);
#[async_trait]
impl BuiltinTool for Count {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        "raw".into()
    }
}

fn setup() -> (Service, HostReceiver, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let partial: PartialToolConfig =
        serde_json::from_value(json!({"source":"builtin", "run":"ask", "result":"edit"})).unwrap();
    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "count".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let (service, host) = Service::new(
        vec![ConfiguredTool {
            definition: ToolDefinition {
                name: "count".into(),
                docs: ToolDocs::default(),
                parameters: json!({"type":"object","properties":{}}),
            },
            config: cfg.conversation.tools.get("count").unwrap(),
            access: Ok(None),
        }],
        Client::default(),
        BuiltinExecutors::new().register("count", Count(count.clone())),
        "/tmp".into(),
        InvocationContext::default(),
    )
    .unwrap();
    (service, host, count)
}

#[tokio::test]
async fn http_call_waits_for_host_release_and_records_edited_result() {
    let (service, mut host, count) = setup();
    let endpoint = Endpoint::start(service).await.unwrap();
    let client = endpoint.connect().await.unwrap();
    let tools = client.peer().list_all_tools().await.unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["count"]
    );
    let peer = client.peer().clone();
    let task =
        tokio::spawn(async move { peer.call_tool(CallToolRequestParams::new("count")).await });
    let Interaction::Prepare {
        arguments, reply, ..
    } = timeout(Duration::from_secs(2), host.recv())
        .await
        .unwrap()
        .unwrap()
        .interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(count.load(Ordering::SeqCst), 0);
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = host.recv().await.unwrap().interaction else {
        panic!("expected release")
    };
    assert_eq!(count.load(Ordering::SeqCst), 0);
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Review { result, reply, .. } = host.recv().await.unwrap().interaction else {
        panic!("expected review")
    };
    assert_eq!(result, Ok("raw".into()));
    reply.send(Ok(Ok("edited".into()))).unwrap();
    let Interaction::Record { result, reply, .. } = host.recv().await.unwrap().interaction else {
        panic!("expected record")
    };
    assert_eq!(result, Ok("edited".into()));
    assert!(!task.is_finished());
    reply.send(Ok(())).unwrap();
    let result = timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.content, vec![Content::text("edited")]);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    client.cancel().await.unwrap();
    endpoint.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_rejects_untrusted_host_and_origin_before_dispatch() {
    let (service, _host, count) = setup();
    let endpoint = Endpoint::start(service).await.unwrap();
    let client = HttpClient::builder().no_proxy().build().unwrap();
    let response = client
        .post(endpoint.url())
        .header("host", "evil.example")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    let response = client
        .post(endpoint.url())
        .header("origin", "https://evil.example")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    endpoint.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_denial_is_recorded_without_execution() {
    let (service, mut host, count) = setup();
    let endpoint = Endpoint::start(service).await.unwrap();
    let client = endpoint.connect().await.unwrap();
    let peer = client.peer().clone();
    let task =
        tokio::spawn(async move { peer.call_tool(CallToolRequestParams::new("count")).await });
    let Interaction::Prepare { reply, .. } = timeout(Duration::from_secs(2), host.recv())
        .await
        .unwrap()
        .unwrap()
        .interaction
    else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Skip {
            reason: "not approved".into(),
        }))
        .unwrap();
    let Interaction::Record {
        reply, raw_result, ..
    } = timeout(Duration::from_secs(2), host.recv())
        .await
        .unwrap()
        .unwrap()
        .interaction
    else {
        panic!("expected recording")
    };
    assert_eq!(raw_result, None);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    reply.send(Ok(())).unwrap();
    let result = timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.content, vec![Content::text("not approved")]);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    client.cancel().await.unwrap();
    endpoint.shutdown().await.unwrap();
}

#[tokio::test]
async fn endpoint_shutdown_cancels_waiting_call_and_closes_listener() {
    let (service, mut host, count) = setup();
    let endpoint = Endpoint::start(service).await.unwrap();
    let url = endpoint.url().to_owned();
    let client = endpoint.connect().await.unwrap();
    let peer = client.peer().clone();
    let task =
        tokio::spawn(async move { peer.call_tool(CallToolRequestParams::new("count")).await });
    let Interaction::Prepare { reply, .. } = timeout(Duration::from_secs(2), host.recv())
        .await
        .unwrap()
        .unwrap()
        .interaction
    else {
        panic!("expected preparation")
    };
    timeout(Duration::from_secs(2), endpoint.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        reply
            .send(Ok(Admission::Run {
                arguments: Map::new()
            }))
            .is_err()
    );
    assert!(
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        HttpClient::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(url)
            .body("{}")
            .send()
            .await
            .unwrap_err()
            .is_connect()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn dropping_endpoint_stops_admission_even_if_host_is_still_connected() {
    let (service, _host, count) = setup();
    let endpoint = Endpoint::start(service).await.unwrap();
    let service = endpoint.service();
    drop(endpoint);
    assert!(matches!(
        service.start_call(CallRequest {
            name: "count".into(),
            arguments: Map::new(),
            correlation: Map::new()
        }),
        Err(ServiceError::Stopped)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}
