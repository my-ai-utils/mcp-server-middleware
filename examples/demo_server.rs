//! Minimal MCP server for manual verification of the middleware.
//!
//! Run:    cargo run --example demo_server
//! Probe:  curl, or `npx @modelcontextprotocol/inspector` pointed at
//!         http://localhost:8081/mcp
//!
//! Exposes one tool (`echo`), one prompt (`greeting`), one static and
//! one dynamic resource. Every 30 seconds it pushes
//! `notifications/resources/updated` for the dynamic resource so
//! `resources/subscribe` can be observed end to end.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use mcp_server_middleware::*;
use my_http_server::async_trait;
use rust_extensions::{ApplicationStates, Logger};
use serde::{Deserialize, Serialize};

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct EchoInput {
    #[property(description = "Text to echo back")]
    pub text: Option<String>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct EchoOutput {
    #[property(description = "The echoed text")]
    pub echoed: String,
}

pub struct EchoTool;

impl ToolDefinition for EchoTool {
    const FUNC_NAME: &'static str = "echo";
    const DESCRIPTION: &'static str = "Echoes the provided text back";
}

#[async_trait::async_trait]
impl McpToolCall<EchoInput, EchoOutput> for EchoTool {
    async fn execute_tool_call(&self, model: EchoInput) -> Result<EchoOutput, String> {
        Ok(EchoOutput {
            echoed: model.text.unwrap_or_else(|| "nothing to echo".to_string()),
        })
    }
}

pub struct GreetingPrompt;

impl PromptDefinition for GreetingPrompt {
    const PROMPT_NAME: &'static str = "greeting";
    const DESCRIPTION: &'static str = "Builds a greeting instruction";

    fn get_argument_descriptions() -> Vec<PromptArgumentDescription> {
        vec![PromptArgumentDescription {
            name: "name".to_string(),
            description: "Who to greet".to_string(),
            required: false,
        }]
    }
}

#[async_trait::async_trait]
impl McpPromptService for GreetingPrompt {
    async fn execute_prompt(
        &self,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptExecutionResult, String> {
        let name = arguments.get("name").map(|s| s.as_str()).unwrap_or("world");
        Ok(PromptExecutionResult {
            description: "A greeting".to_string(),
            message: format!("Say hello to {}!", name),
        })
    }
}

pub struct StaticGreetingResource;

impl ResourceDefinition for StaticGreetingResource {
    const RESOURCE_URI: &'static str = "demo://static/greeting";
    const RESOURCE_NAME: &'static str = "static-greeting";
    const DESCRIPTION: &'static str = "A static greeting resource";
    const MIME_TYPE: &'static str = "text/plain";
}

#[async_trait::async_trait]
impl McpResourceService for StaticGreetingResource {
    async fn read_resource(&self) -> Result<ResourceReadResult, String> {
        Ok(ResourceReadResult {
            contents: vec![ResourceContent {
                uri: Self::RESOURCE_URI.to_string(),
                mime_type: Self::MIME_TYPE.to_string(),
                text: Some("Hello from the static resource!".to_string()),
                blob: None,
            }],
        })
    }
}

const DYNAMIC_RESOURCE_URI: &str = "demo://dynamic/clock";

pub struct DynamicClockResource;

#[async_trait::async_trait]
impl McpResourceService for DynamicClockResource {
    async fn read_resource(&self) -> Result<ResourceReadResult, String> {
        let now = rust_extensions::date_time::DateTimeAsMicroseconds::now();
        Ok(ResourceReadResult {
            contents: vec![ResourceContent {
                uri: DYNAMIC_RESOURCE_URI.to_string(),
                mime_type: "text/plain".to_string(),
                text: Some(format!("Server time: {}", now.to_rfc3339())),
                blob: None,
            }],
        })
    }
}

pub struct DemoAppStates;

impl ApplicationStates for DemoAppStates {
    fn is_initialized(&self) -> bool {
        true
    }

    fn is_shutting_down(&self) -> bool {
        false
    }
}

pub struct DemoLogger;

impl Logger for DemoLogger {
    fn write_info(&self, process: String, message: String, _ctx: Option<HashMap<String, String>>) {
        println!("INFO  [{}] {}", process, message);
    }

    fn write_warning(
        &self,
        process: String,
        message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
        println!("WARN  [{}] {}", process, message);
    }

    fn write_error(
        &self,
        process: String,
        message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
        eprintln!("ERROR [{}] {}", process, message);
    }

    fn write_fatal_error(
        &self,
        process: String,
        message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
        eprintln!("FATAL [{}] {}", process, message);
    }

    fn write_debug_info(
        &self,
        process: String,
        message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
        println!("DEBUG [{}] {}", process, message);
    }
}

/// Prints the two session lifecycle events, so a manual run shows a
/// session appearing (with the client's IP) and going away on `DELETE`
/// or after the idle timeout.
pub struct DemoConnectionInfo;

#[async_trait::async_trait]
impl McpConnectionInfo for DemoConnectionInfo {
    async fn on_connected(&self, session: &McpSession, ctx: &mut my_http_server::HttpContext) {
        println!(
            "MCP session {} connected from {} (protocol {})",
            session.id,
            ctx.request.get_ip().get_real_ip_as_string(),
            session.version
        );
    }

    async fn on_disconnected(&self, session: &McpSession) {
        println!("MCP session {} disconnected", session.id);
    }
}

#[tokio::main]
async fn main() {
    let mut mcp = McpMiddleware::new(
        "/mcp",
        "demo-mcp-server",
        "0.1.0",
        "Demo MCP server exposing an echo tool, a greeting prompt and two resources",
    );

    mcp.register_tool_call(Arc::new(EchoTool));
    mcp.register_prompt(Arc::new(GreetingPrompt));
    mcp.register_resource(Arc::new(StaticGreetingResource));
    mcp.register_connection_info(Arc::new(DemoConnectionInfo));

    let mcp = Arc::new(mcp);

    mcp.register_dynamic_resource(
        DYNAMIC_RESOURCE_URI.to_string(),
        "dynamic-clock".to_string(),
        "Current server time, registered at runtime".to_string(),
        "text/plain".to_string(),
        Arc::new(DynamicClockResource),
    )
    .await;

    let mut http_server = my_http_server::MyHttpServer::new(SocketAddr::from(([0, 0, 0, 0], 8081)));
    http_server.add_middleware(mcp.clone());
    http_server.start(Arc::new(DemoAppStates), Arc::new(DemoLogger));

    // Nudge subscribers periodically so `resources/subscribe` →
    // `notifications/resources/updated` can be observed with curl -N.
    let notifier = mcp.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        tick.tick().await;
        loop {
            tick.tick().await;
            notifier.notify_resource_updated(DYNAMIC_RESOURCE_URI).await;
        }
    });

    println!("MCP demo server listening on http://localhost:8081/mcp");
    tokio::signal::ctrl_c().await.unwrap();
}
