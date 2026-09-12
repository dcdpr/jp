use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use indexmap::IndexMap;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_tool::{Outcome, Question, ToolDefinition, ToolDocs};
use rmcp::{
    ErrorData, ServerHandler,
    model::{CallToolRequestParams, CallToolResult, ServerCapabilities, ServerInfo},
    service::{RequestContext, RoleServer, ServiceExt as _},
};
use serde_json::{Value, json};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use super::{Client, McpServerId};
use crate::{
    Content,
    server::{
        ExecutionOutcome, InvocationContext, builtin::BuiltinExecutors, execute, text_result,
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
    let first = execute(
        &definition,
        "call-1".into(),
        json!({"value":"edited"}),
        &IndexMap::new(),
        &config,
        &client,
        "/work".into(),
        CancellationToken::new(),
        &BuiltinExecutors::new(),
        None,
        &invocation,
        None,
    )
    .await
    .unwrap();
    let ExecutionOutcome::NeedsInput { question, .. } = first else {
        panic!("expected decoded question")
    };
    assert_eq!(question, Question::boolean("confirm", "Continue?").unwrap());
    let answers = IndexMap::from_iter([("confirm".into(), json!(true))]);
    let second = execute(
        &definition,
        "call-1".into(),
        json!({"value":"edited"}),
        &answers,
        &config,
        &client,
        "/work".into(),
        CancellationToken::new(),
        &BuiltinExecutors::new(),
        None,
        &invocation,
        None,
    )
    .await
    .unwrap();
    let ExecutionOutcome::Completed { result, native, .. } = second else {
        panic!("expected final result")
    };
    let native = native.expect("unwrapped text retains native metadata");
    assert_eq!(native.is_error, Some(false));
    assert_eq!(text_result(&native), result);
    assert_eq!(
        serde_json::from_str::<Value>(&result.unwrap()).unwrap(),
        json!({
            "computer.jp/tool":{"name":"actual_tool", "arguments":{"value":"edited"}, "answers":{"confirm":true}, "options":{"limit":7}},
            "computer.jp/context":{"action":"run", "root":"/work", "access":null, "workspace_id":"workspace-1", "conversation_id":"conversation-1"},
            "progressToken":1
        })
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    client.shutdown().await;
    server.cancel().await.unwrap();
}
