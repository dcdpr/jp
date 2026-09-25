use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use indexmap::IndexMap;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
    providers::mcp::{McpProviderConfig, StdioConfig},
};
use jp_tool::{
    Action, ContentBlock, InvocationContext, Outcome, Question, ToolDefinition, ToolDocs,
};
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer, ServiceExt as _},
};
use serde_json::{Map, Value, json};
use tokio::{
    io::duplex,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

use super::{Client, McpServerId};
use crate::{
    Content,
    server::{
        Answers, Execution, ExecutionOutcome,
        builtin::BuiltinExecutors,
        execute,
        http::Endpoint,
        service::{
            Admission, CallRequest, ConfiguredTool, HostReceiver, Interaction, ReleaseDecision,
            Service,
        },
        testing::no_commands,
        tool_definitions,
    },
};

struct Upstream(Arc<AtomicUsize>);

impl ServerHandler for Upstream {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.name, "actual_tool");
        assert_eq!(
            context.meta.0["computer.jp/tool"]["arguments"],
            Value::Object(request.arguments.unwrap())
        );
        let outcome = if context.meta.0["computer.jp/tool"]["answers"]
            .get("confirm")
            .is_some()
        {
            Outcome::Success {
                content: serde_json::to_string(&context.meta.0).unwrap(),
            }
        } else {
            Question::boolean("confirm", "Continue?").unwrap().into()
        };
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&outcome).unwrap(),
        )]))
    }
}

#[tokio::test]
async fn upstream_receives_context_options_and_accumulated_answers() {
    let count = Arc::new(AtomicUsize::new(0));
    let (client_transport, server_transport) = duplex(8192);
    let handler = Upstream(count.clone());
    let server = tokio::spawn(async move { handler.serve(server_transport).await.unwrap() });
    let running = ().serve(client_transport).await.unwrap();
    let server = server.await.unwrap();
    let client = Client::default();
    client
        .services
        .write()
        .await
        .insert(McpServerId::new("upstream"), running);
    let partial: PartialToolConfig =
        serde_json::from_value(json!({"source":"mcp.upstream.actual_tool", "options":{"limit":7}}))
            .unwrap();
    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "alias".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let config = cfg.conversation.tools.get("alias").unwrap();
    let definition = ToolDefinition {
        name: "alias".into(),
        docs: ToolDocs::default(),
        parameters: json!({"type":"object","properties":{"value":{"type":"string"}}}),
    };
    let invocation = InvocationContext {
        workspace_id: "workspace-1".into(),
        conversation_id: "conversation-1".into(),
    };
    let builtins = BuiltinExecutors::new();
    // One context for both attempts, which is what the service does: the second
    // attempt differs only by the answer it carries.
    let execution = Execution {
        definition: &definition,
        id: "call-1".into(),
        arguments: json!({"value":"edited"}),
        action: Action::Run,
        config: &config,
        root: "/work".into(),
        access: None,
        invocation: &invocation,
        builtins: &builtins,
        runner: &no_commands(),
        upstream: &client,
        cancellation: CancellationToken::new(),
        stderr: None,
    };
    let first = execute(&execution, &Answers::new()).await.unwrap();
    let ExecutionOutcome::NeedsInput { question, .. } = first else {
        panic!("expected decoded question")
    };
    assert_eq!(question, Question::boolean("confirm", "Continue?").unwrap());
    let answers = Answers::from_iter([("confirm".into(), json!(true))]);
    let second = execute(&execution, &answers).await.unwrap();
    let ExecutionOutcome::Completed { result, .. } = second else {
        panic!("expected final result")
    };
    assert!(!result.is_error());
    assert_eq!(
        serde_json::from_str::<Value>(&result.to_text()).unwrap(),
        json!({
            "computer.jp/tool":{"name":"actual_tool", "arguments":{"value":"edited"}, "answers":{"confirm":true}, "options":{"limit":7}},
            "computer.jp/context":{"action":"run", "root":"/work", "access":null, "workspace_id":"workspace-1", "conversation_id":"conversation-1"},
            "progressToken":1
        })
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.cancel().await.unwrap();
}

/// A service over `upstream` exposing its `native` tool unattended, as `alias`.
async fn native_service(upstream: &Client) -> (Service, HostReceiver) {
    let mut cfg = AppConfig::new_test();
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "mcp.upstream.native",
        "run": "unattended",
        "result": "unattended",
    }))
    .unwrap();
    cfg.conversation.tools.insert(
        "alias".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let configured = tool_definitions(cfg.conversation.tools.iter(), upstream, None)
        .await
        .unwrap()
        .into_iter()
        .map(|definition| ConfiguredTool {
            config: cfg.conversation.tools.get(&definition.name).unwrap(),
            definition,
            access: Ok(None),
            metadata: Map::new(),
        })
        .collect();
    Service::new(
        configured,
        upstream.clone(),
        BuiltinExecutors::new(),
        no_commands(),
        "/work".into(),
        InvocationContext::default(),
    )
    .unwrap()
}

/// Concurrent plugin Turns share one upstream client.
/// The Turn that finishes first must not take the other Turn's MCP servers down
/// with it.
#[tokio::test]
async fn shutting_down_one_service_leaves_shared_upstream_servers_running() {
    timeout(Duration::from_secs(10), async {
        let count = Arc::new(AtomicUsize::new(0));
        let (client_transport, server_transport) = duplex(8192);
        let handler = NativeUpstream(count.clone());
        let server = tokio::spawn(async move { handler.serve(server_transport).await.unwrap() });
        let running = ().serve(client_transport).await.unwrap();
        let server = server.await.unwrap();
        let upstream = Client::new(IndexMap::from_iter([(
            "upstream".into(),
            McpProviderConfig::Stdio(StdioConfig {
                command: "unused-fixture".into(),
                arguments: vec![],
                variables: vec![],
                checksum: None,
                optional: false,
                startup_timeout_secs: 60,
            }),
        )]));
        upstream
            .services
            .write()
            .await
            .insert(McpServerId::new("upstream"), running);

        let (finished, _finished_host) = native_service(&upstream).await;
        let (surviving, mut host) = native_service(&upstream).await;
        finished.shutdown().await;

        let call = surviving
            .start_call(CallRequest {
                name: "alias".into(),
                arguments: Map::new(),
                correlation: Map::new(),
            })
            .unwrap();
        let Interaction::Prepare {
            arguments, reply, ..
        } = host.recv().await.unwrap().interaction
        else {
            panic!("expected preparation")
        };
        reply.send(Ok(Admission::Run { arguments })).unwrap();
        let Interaction::Release { reply, .. } = host.recv().await.unwrap().interaction else {
            panic!("expected release")
        };
        reply.send(Ok(ReleaseDecision::Execute)).unwrap();
        let Interaction::Record { reply, .. } = host.recv().await.unwrap().interaction else {
            panic!("expected recording")
        };
        reply.send(Ok(())).unwrap();

        assert_eq!(call.finish().await.unwrap().to_text(), "alpha\n\nresource");
        assert_eq!(count.load(Ordering::SeqCst), 1);

        surviving.shutdown().await;
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

struct NativeUpstream(Arc<AtomicUsize>);

impl ServerHandler for NativeUpstream {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![Tool::new(
                "native",
                "Native result",
                Arc::new(
                    json!({"type":"object","properties":{}})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        assert_eq!(request.name, "native");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::from_value(json!({
            "content":[{"type":"text","text":"alpha"},{"type":"image","data":"AA==","mimeType":"image/png"},{"type":"resource","resource":{"uri":"fixture:///resource","text":"resource","mimeType":"text/plain"}}],
            "isError":false,"structuredContent":{"answer":42},"_meta":{"fixture/source":"upstream"}
        })).unwrap())
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "One upstream call read end to end, from its wire result to the caller's"
)]
async fn native_upstream_result_survives_host_projection_and_http_delivery() {
    timeout(Duration::from_secs(10), async {
        let count = Arc::new(AtomicUsize::new(0));
        // Use the stdio codec without starting an extra fixture executable.
        let (client_transport, server_transport) = duplex(8192);
        let handler = NativeUpstream(count.clone());
        let server = tokio::spawn(async move { handler.serve(server_transport).await.unwrap() });
        let running = ().serve(client_transport).await.unwrap();
        let server = server.await.unwrap();

        let upstream = Client::new(IndexMap::from_iter([(
            "upstream".into(),
            McpProviderConfig::Stdio(StdioConfig {
                command: "unused-fixture".into(),
                arguments: vec![],
                variables: vec![],
                checksum: None,
                optional: false,
                startup_timeout_secs: 60,
            }),
        )]));
        upstream
            .services
            .write()
            .await
            .insert(McpServerId::new("upstream"), running);

        let mut cfg = AppConfig::new_test();
        let partial: PartialToolConfig = serde_json::from_value(json!({
            "source": "mcp.upstream.native",
            "run": "unattended",
            "result": "ask",
        }))
        .unwrap();
        cfg.conversation.tools.insert(
            "alias".into(),
            ToolConfig::from_partial(partial, vec![]).unwrap(),
        );
        let definitions = tool_definitions(cfg.conversation.tools.iter(), &upstream, None)
            .await
            .unwrap();
        let configured = definitions
            .into_iter()
            .map(|definition| ConfiguredTool {
                config: cfg.conversation.tools.get(&definition.name).unwrap(),
                definition,
                access: Ok(None),
                metadata: Map::new(),
            })
            .collect();
        let (service, mut host) = Service::new(
            configured,
            upstream,
            BuiltinExecutors::new(),
            no_commands(),
            "/work".into(),
            InvocationContext::default(),
        )
        .unwrap();

        let endpoint = Endpoint::start(service).await.unwrap();
        let client = endpoint.connect().await.unwrap();
        let peer = client.peer().clone();
        let result = tokio::spawn(async move {
            peer.call_tool(CallToolRequestParams::new("alias"))
                .await
                .unwrap()
        });

        let Interaction::Prepare {
            arguments, reply, ..
        } = host.recv().await.unwrap().interaction
        else {
            panic!("expected preparation")
        };
        reply.send(Ok(Admission::Run { arguments })).unwrap();

        let Interaction::Release { reply, .. } = host.recv().await.unwrap().interaction else {
            panic!("expected release")
        };
        reply.send(Ok(ReleaseDecision::Execute)).unwrap();

        // Everything the upstream server sent reaches the Host intact: the
        // image block, the structured data, and the result metadata.
        let Interaction::Review {
            result: reviewed,
            reply,
            ..
        } = host.recv().await.unwrap().interaction
        else {
            panic!("expected review")
        };
        assert!(matches!(
            &reviewed.content[1],
            ContentBlock::Image(image)
                if image.data == "AA==" && image.mime_type == "image/png"
        ));
        assert_eq!(reviewed.structured_content, Some(json!({"answer": 42})));
        assert_eq!(
            reviewed.metadata,
            Some(
                json!({"fixture/source": "upstream"})
                    .as_object()
                    .unwrap()
                    .clone()
            )
        );
        reply.send(Ok(reviewed.clone())).unwrap();

        let Interaction::Record { recording, reply } = host.recv().await.unwrap().interaction
        else {
            panic!("expected recording")
        };
        assert_eq!(recording.result, reviewed);
        assert_eq!(recording.raw_result, Some(reviewed));
        // The conversation stores only the text, which is what makes the
        // assertion below worth making.
        assert_eq!(recording.result.to_text(), "alpha\n\nresource");
        assert!(!reply.is_closed());
        reply.send(Ok(())).unwrap();

        assert_eq!(
            serde_json::to_value(result.await.unwrap()).unwrap(),
            json!({
                "content": [
                    {"type": "text", "text": "alpha"},
                    {"type": "image", "data": "AA==", "mimeType": "image/png"},
                    {"type": "resource", "resource": {
                        "uri": "fixture:///resource",
                        "text": "resource",
                        "mimeType": "text/plain",
                    }},
                ],
                "isError": false,
                "structuredContent": {"answer": 42},
                "_meta": {"fixture/source": "upstream"},
            })
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);

        client.cancel().await.unwrap();
        endpoint.shutdown().await.unwrap();
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}
