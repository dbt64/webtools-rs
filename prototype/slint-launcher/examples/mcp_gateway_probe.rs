#[path = "../src/mcp_http.rs"]
mod mcp_http;
use rmcp::{model::*, service::RequestContext, ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use std::{borrow::Cow, time::Duration};

#[derive(Clone)]
struct Probe;
impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
        ])
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(serde_json::from_value(json!({"tools":[{"name":"probe.echo","description":"Isolated protocol probe","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]})).unwrap())
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(CallToolResult::structured(json!({"text":request.arguments.and_then(|a| a.get("text").cloned()).unwrap_or_default()})).into())
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let token = std::env::var("WT_MCP_PROBE_TOKEN")?;
    let router = mcp_http::router(Probe, token, address.to_string());
    println!("{}", json!({"url":format!("http://{address}/mcp")}));
    axum::serve(listener, router).await?;
    Ok(())
}
