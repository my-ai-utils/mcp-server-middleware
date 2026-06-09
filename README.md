# MCP Server Middleware

A Rust middleware library for implementing Model Context Protocol (MCP) servers. This middleware handles MCP protocol communication, session management, and tool call execution, making it easy to build MCP-compatible servers for any use case.

The middleware provides a flexible, trait-based architecture that allows you to implement custom tool calls for any domain - whether it's database access, file operations, API integrations, or any other functionality you want to expose through the MCP protocol.

## About Model Context Protocol (MCP)

The Model Context Protocol (MCP) is a standardized protocol that enables AI applications to securely access external data sources and tools. MCP provides a unified interface for AI agents to interact with external systems, databases, APIs, and services. This middleware implements the Streamable HTTP transport and negotiates protocol revisions `2025-03-26`, `2025-06-18` and `2025-11-25` (an unknown requested version is answered with the latest supported one).

### Core Concepts

MCP servers expose capabilities through three main mechanisms:

1. **Tools**: Executable functions that AI agents can call to perform actions
   - Examples: Execute SQL queries, read/write files, call REST APIs, run shell commands
   - Each tool has a name, description, and JSON schema defining input/output types
   - Tools are discovered via `tools/list` and executed via `tools/call`

2. **Prompts**: Pre-configured prompt templates with variable substitution
   - Help guide AI interactions with structured, reusable prompts
   - Support required and optional arguments for customization
   - Discovered via `prompts/list` and retrieved via `prompts/get` with arguments

3. **Resources**: Data sources that AI agents can read for context
   - Examples: Files, database schemas, documentation, configuration files
   - Each resource has a URI, name, description, MIME type, and optional metadata
   - Support pagination for large resource lists
   - Discovered via `resources/list` and read via `resources/read`

### Protocol Architecture

**Transport Layer**: MCP uses JSON-RPC 2.0 over HTTP with Server-Sent Events (SSE) for streaming responses. This provides:
- Standardized request/response format
- Real-time streaming capabilities
- Compatibility with existing HTTP infrastructure

**Session Management**: 
- Each client connection establishes a session via `initialize` request
- Sessions are identified by unique session IDs returned in `mcp-session-id` header
- Subsequent requests require the session ID for authentication
- GET requests establish SSE streams for server-to-client notifications

**Capability Discovery**:
- Servers declare capabilities during initialization (tools, prompts, resources)
- Clients discover available capabilities through list endpoints
- Dynamic schema generation ensures clients always have up-to-date tool definitions

**Error Handling**:
- Standard JSON-RPC error codes (-32002 for resource not found, -32603 for internal errors)
- Structured error responses with error codes, messages, and optional data

### This Implementation

This middleware (`mcp-server-middleware`) is a **Rust library** that provides a complete, production-ready implementation of the MCP protocol specification. It offers:

**Trait-Based Architecture**:
- `McpToolCall<Input, Output>`: Trait for implementing tool execution logic
- `ToolDefinition`: Trait for providing tool metadata (name, description)
- `McpPromptService`: Trait for implementing prompt templates
- `PromptDefinition`: Trait for providing prompt metadata
- `ResourceDefinition` & `McpResourceService`: Traits for resource management (static, compile-time URIs)
- Dynamic resource registry: register/unregister resources with runtime URIs after the middleware is mounted

**Type Safety**:
- Automatic JSON schema generation from Rust types using `ApplyJsonSchema` macro
- Compile-time type checking ensures schemas match implementation
- Support for dynamic enum values based on runtime data

**Protocol Compliance**:
- Streamable HTTP transport; protocol revisions `2025-03-26` / `2025-06-18` / `2025-11-25` with version negotiation at `initialize`
- All required protocol methods (`initialize`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, `resources/list`, `resources/read`, `resources/templates/list`, `resources/subscribe`, `resources/unsubscribe`, `ping`)
- Notifications (`notifications/*`) accepted with `202`; unknown request methods answered with JSON-RPC `-32601`
- Proper JSON-RPC 2.0 formatting, including string request ids and spec-shaped `error: {code, message}` objects
- SSE streaming support with keepalives on both the GET notification stream and long `tools/call` responses
- Session management with secure session IDs, `404` for expired sessions (clients auto-reinitialize) and background GC for abandoned sessions

**Integration**:
- Seamless integration with `my-http-server` as HTTP middleware
- Easy registration of tools, prompts, and resources
- Automatic handling of protocol details (session management, error formatting, schema generation)

**Key Features**:
- Zero-boilerplate tool registration - just implement traits and register
- Automatic schema generation - no manual JSON schema writing
- Session-based security - each client gets isolated session
- Streaming support - real-time updates via SSE
- Resource pagination - efficient handling of large resource lists
- Dynamic resources - register/unregister resources at runtime (e.g. one per uploaded file or generated artifact), served as `blob` (base64) or `text`
- Prompt templates - reusable prompts with variable substitution

## Features

* **MCP Protocol Support**: Full implementation of MCP protocol including initialization, tool calls, prompts, and notifications
* **Session Management**: Automatic session creation and management with session-based authentication
* **Tool Call Framework**: Easy-to-use trait-based system for implementing custom tool calls
* **Prompt Support**: Register and expose prompts that MCP clients can discover and use
* **Resource Support**: Expose data sources (files, schemas, etc.) that clients can read for context
* **HTTP Integration**: Seamless integration with `my-http-server` as middleware
* **Type-Safe Tool Definitions**: Leverages `my-ai-agent` for type-safe JSON schema generation
* **Dynamic Enumeration**: Support for dynamically generated enum values based on runtime data
* **Elicitation** (server→client user input): tools that implement `McpToolCallEx` can request a value from the user mid-execution via `ToolCallContext::elicit()`. Requires the client to advertise `capabilities.elicitation` at initialize. Useful for credentials and confirmations that should never enter the LLM context.

## Installation

Add the dependency to your `Cargo.toml`:

```toml
[dependencies]
mcp-server-middleware = { git = "https://github.com/my-ai-utils/mcp-server-middleware.git" }
my-http-server = { tag = "0.8.3", git = "https://github.com/MyJetTools/my-http-server.git"}
my-ai-agent = { tag = "0.1.0", git = "https://github.com/my-ai-utils/my-ai-agent.git", features = ["agent"] }
tokio = { version = "*", features = ["full"] }
serde = { version = "*", features = ["derive"] }
serde_json = "*"
async-trait = "*"
```

## Quick Start

A complete runnable example lives in [`examples/demo_server.rs`](examples/demo_server.rs) — one tool, one prompt, a static and a dynamic resource, plus a periodic `notify_resource_updated` trigger:

```bash
cargo run --example demo_server
# then probe http://localhost:8081/mcp with curl or `npx @modelcontextprotocol/inspector`
```

### 1. Create the Middleware

Create an instance of `McpMiddleware` with your server configuration:

```rust
use mcp_server_middleware::McpMiddleware;
use std::sync::Arc;

let mut mcp_middleware = McpMiddleware::new(
    "/mcp",                         // MCP endpoint path
    "My MCP Server",                // Server name
    "0.1.0",                        // Server version
    "Instructions for using this MCP server", // Instructions
);
```

### 2. Implement a Tool Service

Create a service that implements the `McpToolCall` trait:

```rust
use mcp_server_middleware::{McpToolCall, ToolDefinition};
use my_ai_agent::{macros::ApplyJsonSchema, json_schema::*};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use std::sync::Arc;

// Define your input and output types with JSON schema
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolRequest {
    #[property(description = "Input parameter description")]
    pub input_field: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolResponse {
    #[property(description = "Output parameter description")]
    pub output_field: String,
}

// Create your handler struct
pub struct MyToolHandler {
    // Add any dependencies you need (e.g., app context, database connection, etc.)
}

impl MyToolHandler {
    pub fn new() -> Self {
        Self {}
    }
}

// Implement ToolDefinition to provide metadata
impl ToolDefinition for MyToolHandler {
    const FUNC_NAME: &'static str = "my_tool";
    const DESCRIPTION: &'static str = "Description of what this tool does";
}

// Implement McpToolCall to handle tool execution
#[async_trait::async_trait]
impl McpToolCall<MyToolRequest, MyToolResponse> for MyToolHandler {
    async fn execute_tool_call(
        &self,
        request: MyToolRequest,
    ) -> Result<MyToolResponse, String> {
        // Your implementation here
        let result = format!("Processed: {}", request.input_field);
        
        Ok(MyToolResponse {
            output_field: result,
        })
    }
}
```

### 3. Register Tool Calls

Register your service with the middleware:

```rust
let service = Arc::new(MyToolHandler::new());
mcp_middleware.register_tool_call(service);
```

### 4. Register Prompts (Optional)

You can also register prompts that MCP clients can discover and use:

```rust
use mcp_server_middleware::{McpPromptService, PromptDefinition};
use std::collections::HashMap;
use async_trait::async_trait;

// Implement the prompt service
pub struct MyPromptService;

impl PromptDefinition for MyPromptService {
    const PROMPT_NAME: &'static str = "example_prompt";
    const DESCRIPTION: &'static str = "An example prompt that demonstrates prompt functionality";
    
    fn get_argument_descriptions() -> Vec<mcp_server_middleware::PromptArgumentDescription> {
        vec![
            mcp_server_middleware::PromptArgumentDescription {
                name: "variable_name".to_string(),
                description: "Description of what this variable represents".to_string(),
                required: true,
            },
            mcp_server_middleware::PromptArgumentDescription {
                name: "optional_param".to_string(),
                description: "An optional parameter".to_string(),
                required: false,
            },
        ]
    }
}

#[async_trait]
impl McpPromptService for MyPromptService {
    async fn execute_prompt(
        &self,
        arguments: &HashMap<String, String>,
    ) -> Result<mcp_server_middleware::PromptExecutionResult, String> {
        let var_value = arguments.get("variable_name")
            .ok_or("variable_name is required")?;
        
        Ok(mcp_server_middleware::PromptExecutionResult {
            description: "Example prompt result".to_string(),
            message: format!("Processing with variable: {}", var_value),
        })
    }
}

// Register the prompt
let prompt_service = Arc::new(MyPromptService);
mcp_middleware.register_prompt(prompt_service);
```

### 5. Register Resources (Optional)

Resources allow clients to read data sources. Implement a resource service:

```rust
use mcp_server_middleware::{McpResourceService, ResourceDefinition, ResourceReadResult, ResourceContent};
use async_trait::async_trait;

pub struct MyResourceService;

impl ResourceDefinition for MyResourceService {
    const RESOURCE_URI: &'static str = "file:///example.txt";
    const RESOURCE_NAME: &'static str = "example.txt";
    const DESCRIPTION: &'static str = "An example resource file";
    const MIME_TYPE: &'static str = "text/plain";
    
    // Optional: Override for additional metadata
    fn get_title(&self) -> Option<&str> {
        Some("Example Resource")
    }
    
    fn get_size(&self) -> Option<u64> {
        Some(1024) // Size in bytes
    }
}

#[async_trait]
impl McpResourceService for MyResourceService {
    async fn read_resource(&self) -> Result<ResourceReadResult, String> {
        // Read your resource content here
        let content = "Resource content here".to_string();
        
        Ok(ResourceReadResult {
            contents: vec![ResourceContent {
                uri: Self::RESOURCE_URI.to_string(),
                mime_type: "text/plain".to_string(),
                text: Some(content),
                blob: None,
            }],
        })
    }
}

// Register the resource
let resource_service = Arc::new(MyResourceService);
mcp_middleware.register_resource(resource_service);
```

### 5b. Register Dynamic Resources (Runtime)

`ResourceDefinition` pins the URI to a `const &'static str`, so it can
only describe resources known at compile time. For resources minted at
runtime — one per uploaded file, per database row, per generated
artifact — use the dynamic registry. The middleware keeps it behind a
`RwLock`, so you can register/unregister even after the middleware is
wrapped in `Arc` and mounted on the HTTP server. `resources/list`,
`resources/read`, `resources/subscribe`, and the `initialize`
capability advertisement all fan out across both the static and the
dynamic registries.

```rust
use mcp_server_middleware::{McpResourceService, ResourceReadResult, ResourceContent};
use async_trait::async_trait;
use std::sync::Arc;

pub struct BlobResource {
    uri: String,
    bytes_base64: String,
    mime_type: String,
}

#[async_trait]
impl McpResourceService for BlobResource {
    async fn read_resource(&self) -> Result<ResourceReadResult, String> {
        Ok(ResourceReadResult {
            contents: vec![ResourceContent {
                uri: self.uri.clone(),
                mime_type: self.mime_type.clone(),
                text: None,
                blob: Some(self.bytes_base64.clone()), // base64-encoded payload
            }],
        })
    }
}

// `mcp_middleware: Arc<McpMiddleware>` — fine to call after it's mounted.
let uri = format!("app://blob/{id}");
let svc = Arc::new(BlobResource {
    uri: uri.clone(),
    bytes_base64,
    mime_type: "image/png".to_string(),
});

// Minimal form:
mcp_middleware
    .register_dynamic_resource(
        uri.clone(),
        "blob name".to_string(),
        "Generated blob".to_string(),
        "image/png".to_string(),
        svc.clone(),
    )
    .await;

// Or with optional title / size / icons:
mcp_middleware
    .register_dynamic_resource_full(
        uri.clone(),
        "blob name".to_string(),
        "Generated blob".to_string(),
        "image/png".to_string(),
        Some("Blob Title".to_string()),
        Some(4096),       // size in bytes
        Vec::new(),       // icons
        svc,
    )
    .await;

// Push the change to live MCP sessions:
mcp_middleware.notify_resources_changed().await;

// Remove it later (true if it was present):
let _ = mcp_middleware.unregister_dynamic_resource(&uri).await;
```

Notes:
- Registering the same URI twice overwrites the previous entry.
- The dynamic registry is unpaginated; `resources/list` surfaces every
  dynamic resource on the page after the static ones are exhausted.
  Intended for "tens to low thousands" of entries.
- Returning `blob` (base64) with an image MIME type lets MCP clients
  render the resource as an image content block — the right channel for
  binary payloads, instead of stuffing base64 into tool-call JSON.

### 6. Integrate with HTTP Server

Add the middleware to your HTTP server:

```rust
use my_http_server::MyHttpServer;
use std::net::SocketAddr;

let mut http_server = MyHttpServer::new(SocketAddr::from(([0, 0, 0, 0], 8005)));
let mcp_middleware = Arc::new(mcp_middleware);
http_server.add_middleware(mcp_middleware);
http_server.start(app_states, logger);
```

## Returning instructions from a tool call

By default a tool call returns just structured data — the model receives it via the `structuredContent` field of the `tools/call` response. Sometimes a tool wants to attach a short *inline instruction* for the model on top of the data: how to interpret the result, what to do next, what to ask the user. The middleware exposes this through `ToolCallOutput<T>` and a second trait `McpToolCallWithInstruction<Input, Output>`.

`McpToolCall<Input, Output>` is unchanged — existing implementations keep working without any edits. To use the new feature, implement `McpToolCallWithInstruction` instead and return a `ToolCallOutput`:

```rust
use mcp_server_middleware::{
    McpToolCallWithInstruction, ToolCallOutput, ToolDefinition,
};

#[async_trait::async_trait]
impl McpToolCallWithInstruction<MyReq, MyResp> for MyHandler {
    async fn execute_tool_call_with_instruction(
        &self,
        req: MyReq,
    ) -> Result<ToolCallOutput<MyResp>, String> {
        let resp = MyResp { items: vec![/* ... */] };

        if resp.items.is_empty() {
            return Ok(ToolCallOutput::with_instruction(
                resp,
                "Result is empty. Suggest the user widen the filter.",
            ));
        }

        Ok(ToolCallOutput::new(resp))
    }
}
```

`ToolCallOutput` constructors:
- `ToolCallOutput::new(data)` — data only, no instruction (equivalent to the legacy `Ok(data)` behavior).
- `ToolCallOutput::with_instruction(data, text)` — data plus an inline instruction for the model.
- `From<T> for ToolCallOutput<T>` is implemented, so `data.into()` works as a shortcut for `ToolCallOutput::new(data)`.

`McpToolCallWithInstruction` is wired through a blanket impl over `McpToolCall`, so any existing `McpToolCall` implementation is automatically a `McpToolCallWithInstruction` that returns `ToolCallOutput::new(data)`. You only implement the new trait directly when you want to attach an instruction. Registration uses the same `register_tool_call(...)` method.

### How the instruction reaches the model

When a tool returns an instruction, the `tools/call` response includes both:

- `result.structuredContent` — the structured `data` (same as before);
- `result.content[0]` — `{ "type": "text", "text": "<instruction>" }`, which is the standard MCP channel models read.

When `instruction` is `None`, behavior is unchanged from previous versions: `content[0].text` carries the JSON-stringified data and `structuredContent` carries the same data structurally.

### Server-level instructions vs per-call instructions

These are two distinct mechanisms — do not confuse them:

- **Server instructions** — the `instructions` argument of `McpMiddleware::new(path, name, version, instructions)`. They are returned once during `initialize` and apply to the entire session. Use them for global guidance ("this server exposes a Postgres database, queries should be read-only").
- **Per-call instructions** — `ToolCallOutput::with_instruction(...)`. They are returned in the response to a specific `tools/call` and are scoped to that call's context. Use them for situational hints that depend on the actual tool result ("the search returned no rows — propose to widen the filter").

## Server→client elicitation (asking the user for input)

Sometimes a tool needs a value the model **must not** see — a database password, a 2FA code, an explicit confirmation for a destructive action. MCP calls this *elicitation*: mid-tool-call the server sends a JSON-RPC request back over the SSE stream asking the connected client to prompt the user. The user's answer is returned to the tool; only the tool's final result reaches the model.

This is implemented as a **separate trait** — `McpToolCallEx` — and a **separate registration method** — `register_tool_call_with_context`. Plain `McpToolCall` impls are untouched.

### Client-side prerequisite

The connected MCP client must advertise the `elicitation` capability during `initialize`:

```json
{
  "capabilities": {
    "elicitation": {}
  }
}
```

If it did not, `ctx.elicit(...)` returns `Err("MCP client does not support elicitation")`. Many clients (and ad-hoc `curl`-style integrations) don't support elicitation — always handle the error path.

### Implementing a context-aware tool

Implement `McpToolCallEx` instead of `McpToolCall`. The execute method receives `&ToolCallContext` alongside the input:

```rust
use std::sync::Arc;
use std::time::Duration;

use mcp_server_middleware::{
    ElicitationAction, McpToolCallEx, ToolCallContext, ToolDefinition,
};
use my_ai_agent::macros::ApplyJsonSchema;
use serde::{Deserialize, Serialize};

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ConnectDbRequest {
    #[property(description = "Database host, e.g. db.internal:5432")]
    pub host: String,
    #[property(description = "Database name")]
    pub database: String,
    #[property(description = "DB user")]
    pub user: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ConnectDbResponse {
    #[property(description = "Server version reported by the database")]
    pub server_version: String,
}

pub struct ConnectDbHandler;

impl ToolDefinition for ConnectDbHandler {
    const FUNC_NAME: &'static str = "connect_db";
    const DESCRIPTION: &'static str =
        "Connect to a Postgres database. The password is asked from the user and never enters the model context.";
}

#[async_trait::async_trait]
impl McpToolCallEx<ConnectDbRequest, ConnectDbResponse> for ConnectDbHandler {
    async fn execute_tool_call(
        &self,
        req: ConnectDbRequest,
        ctx: &ToolCallContext,
    ) -> Result<ConnectDbResponse, String> {
        // Flat JSON schema for what we want from the user.
        // MCP elicitation only supports flat objects of primitive
        // properties (string / number / integer / boolean / enum).
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "password": {
                    "type": "string",
                    "description": format!("Password for {}@{}", req.user, req.host),
                    "format": "password",
                },
            },
            "required": ["password"],
        });

        let resp = ctx
            .elicit("Enter database password", schema, Duration::from_secs(60))
            .await?;

        match resp.action {
            ElicitationAction::Accept => {
                let password = resp
                    .content
                    .as_ref()
                    .and_then(|c| c.get("password"))
                    .and_then(|v| v.as_str())
                    .ok_or("Elicitation accepted but `password` is missing")?
                    .to_string();

                let version = open_pg_connection(&req, &password).await?;
                Ok(ConnectDbResponse { server_version: version })
            }
            ElicitationAction::Decline => {
                Err("User declined to share the password".to_string())
            }
            ElicitationAction::Cancel => {
                Err("Elicitation cancelled".to_string())
            }
        }
    }
}

// Registration uses `register_tool_call_with_context` — NOT `register_tool_call`.
mcp_middleware.register_tool_call_with_context(Arc::new(ConnectDbHandler));
```

### What `ToolCallContext` exposes

```rust
pub struct ToolCallContext {
    pub session_id: String,         // mcp-session-id of the originating call
    pub supports_elicitation: bool, // did the client advertise the capability?
    // ...plus internal handles to the elicitation registry and the SSE sender
}

impl ToolCallContext {
    pub async fn elicit(
        &self,
        message: &str,
        requested_schema: serde_json::Value,
        timeout: Duration,
    ) -> Result<ElicitationResponse, String>;
}
```

What `elicit(...)` does under the hood:

1. Allocates a **negative** request id (negative on purpose — never collides with ids the client allocates for its own requests).
2. Pushes an `elicitation/create` JSON-RPC request onto the SSE stream for this session: `{ id, message, requestedSchema }`.
3. Parks the call on a `oneshot` keyed by that id, with the supplied timeout.
4. When the client POSTs back a response with the same `id`, the middleware parses it into an `ElicitationResponse` and wakes the parked call.

The error path:
- `"MCP client does not support elicitation"` — client didn't advertise `capabilities.elicitation` at `initialize`.
- `"No active SSE channel for this MCP session"` — client opened a session but did not keep its `GET /mcp` SSE stream open.
- `"Failed to deliver elicitation/create — SSE channel closed"` — the SSE stream died between allocation and send.
- `"Elicitation timed out — client did not reply in time"` — timeout elapsed before the client responded.

You can bubble these straight up with `?`, or remap them to a tool-specific message before returning.

### The reply shape

```rust
pub enum ElicitationAction { Accept, Decline, Cancel }

pub struct ElicitationResponse {
    pub action: ElicitationAction,
    /// Present when `action == Accept`. JSON object matching the
    /// `requested_schema` you sent. `None` for Decline / Cancel.
    pub content: Option<serde_json::Value>,
}
```

Per the MCP spec:
- `Accept` — user provided values. `content` is a JSON object matching the schema; pull fields out with `content.get("field_name")`.
- `Decline` — user actively refused (e.g. clicked "Don't share"). Treat as a *refusal*, not an internal error — return a clean explanation to the model.
- `Cancel` — user dismissed the prompt (closed the dialog, switched away). Usually treat the same as `Decline`; you can distinguish if your UX cares.

If the client returns a malformed or error payload, the middleware coerces it into `Cancel` with `content == None` — so a `Cancel` branch covers both "user cancelled" and "client crashed."

### Inline instructions on the context-aware path

To combine elicitation with an inline instruction, implement `McpToolCallExWithInstruction` instead of `McpToolCallEx` — same signature, but it returns `ToolCallOutput<OutputData>`, so `ToolCallOutput::with_instruction(...)` works exactly like on the plain path:

```rust
#[async_trait::async_trait]
impl McpToolCallExWithInstruction<MyInput, MyOutput> for MyTool {
    async fn execute_tool_call_with_instruction(
        &self,
        model: MyInput,
        ctx: &ToolCallContext,
    ) -> Result<ToolCallOutput<MyOutput>, String> {
        // ... ctx.elicit(...) as usual ...
        Ok(ToolCallOutput::with_instruction(output, "Hint for the model"))
    }
}
```

Every `McpToolCallEx` implementor gets `McpToolCallExWithInstruction` for free through a blanket impl (with `instruction = None`), so existing tools keep working unchanged. `register_tool_call_with_context` accepts both.

## Dynamic Enum Fields

For tool call parameters that need to accept values from a dynamically generated list (such as filtering by available cities, countries, or other runtime-determined options), you can use dynamic enumeration. This feature allows enum values to be generated at runtime based on your application's current data state.

To use dynamic enums, specify the `enum` parameter in the `#[property]` attribute with the name of an async function that will generate the enum values. This function must return `Option<Vec<StrOrString<'static>>>` and will be called automatically when the MCP client requests the tool schema.

```rust
use my_ai_agent::macros::ApplyJsonSchema;
use serde::{Deserialize, Serialize};
use service_sdk::rust_extensions::StrOrString;

#[derive(ApplyJsonSchema, Serialize, Deserialize, Debug)]
pub struct FilterPropertiesToolCallModel {
    #[property(enum: "get_city_enum", description: "Filter properties by city location")]
    pub city: Option<String>,

    #[property(enum: "get_country_enum", description: "Filter by country using ISO2 code")]
    pub country: Option<String>,

    #[property(enum: "get_project_name_enum", description: "Filter by development project name")]
    pub project_name: Option<String>,
}

// Implement the enum generation functions
async fn get_city_enum() -> Option<Vec<StrOrString<'static>>> {
    let data_access = DATA_HOLDER.read().await;
    data_access.units.group_by_project(|unit| &unit.city)
}

async fn get_country_enum() -> Option<Vec<StrOrString<'static>>> {
    let data_access = DATA_HOLDER.read().await;
    data_access.units.group_by_project(|unit| &unit.country)
}

async fn get_project_name_enum() -> Option<Vec<StrOrString<'static>>> {
    let data_access = DATA_HOLDER.read().await;
    data_access.units.group_by_project(|project| &project.title)
}
```

The enum functions are automatically discovered and called when generating the JSON schema for your tool. The returned values will be included in the tool's input schema as enum constraints, providing clients with the available options for each parameter. This is particularly useful for parameters that depend on your application's current state, such as filtering by available cities, selecting from active projects, or choosing from dynamically loaded configuration options.

## Creating Tool Calls and Prompts

### Step-by-Step Guide for Tool Calls

1. **Create the tool call file** (e.g., `my_tool_call.rs`)

2. **Define Input and Output Structures** with `ApplyJsonSchema`:

```rust
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolInputData {
    #[property(description = "Description of the parameter")]
    pub parameter_name: String,
    
    #[property(description = "Another parameter")]
    pub another_param: Option<i32>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolResponse {
    #[property(description = "Result description")]
    pub result: String,
    
    #[property(description = "Status code")]
    pub status: i32,
}
```

3. **Create the Handler Struct**:

```rust
pub struct MyToolHandler {
    // Add dependencies if needed (e.g., app context)
}

impl MyToolHandler {
    pub fn new() -> Self {
        Self {}
    }
}
```

4. **Implement `ToolDefinition` Trait**:

```rust
impl ToolDefinition for MyToolHandler {
    const FUNC_NAME: &'static str = "my_tool_name";
    const DESCRIPTION: &'static str = "Clear description of what this tool does";
}
```

5. **Implement `McpToolCall` Trait**:

```rust
#[async_trait::async_trait]
impl McpToolCall<MyToolInputData, MyToolResponse> for MyToolHandler {
    async fn execute_tool_call(
        &self,
        model: MyToolInputData,
    ) -> Result<MyToolResponse, String> {
        // Your implementation here
        let result = MyToolResponse {
            result: "Success".to_string(),
            status: 200,
        };
        
        Ok(result)
    }
}
```

6. **Register in your startup code**:

```rust
mcp_middleware.register_tool_call(Arc::new(MyToolHandler::new()));
```

### Step-by-Step Guide for Prompts

1. **Create the prompt handler struct**:

```rust
pub struct MyPromptHandler;
```

2. **Implement `PromptDefinition` Trait**:

```rust
impl PromptDefinition for MyPromptHandler {
    const PROMPT_NAME: &'static str = "my_prompt_name";
    const DESCRIPTION: &'static str = "Description of what this prompt provides";
    
    fn get_argument_descriptions() -> Vec<PromptArgumentDescription> {
        vec![
            PromptArgumentDescription {
                name: "param1".to_string(),
                description: "Description of param1".to_string(),
                required: true,
            },
            PromptArgumentDescription {
                name: "param2".to_string(),
                description: "Description of param2".to_string(),
                required: false,
            },
        ]
    }
}
```

3. **Implement `McpPromptService` Trait**:

```rust
#[async_trait::async_trait]
impl McpPromptService for MyPromptHandler {
    async fn execute_prompt(
        &self,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptExecutionResult, String> {
        // Access arguments if needed
        let param1 = arguments.get("param1");
        
        // Build your prompt content
        let prompt_content = format!(
            r#"
# Your Prompt Title

## Section 1
Content here...
"#
        );
        
        let result = PromptExecutionResult {
            description: "What this prompt provides".to_string(),
            message: prompt_content,
        };
        
        Ok(result)
    }
}
```

4. **Register in your startup code**:

```rust
mcp_middleware.register_prompt(Arc::new(MyPromptHandler));
```

## Complete Example: Postgres MCP Server

The following example demonstrates a real-world implementation - a Postgres MCP server that allows AI agents to execute SQL queries. This serves as a concrete reference for building your own MCP servers:

```rust
use std::sync::Arc;
use mcp_server_middleware::{McpMiddleware, McpToolCall, ToolDefinition};
use my_http_server::MyHttpServer;
use my_ai_agent::{macros::ApplyJsonSchema, json_schema::*};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use std::net::SocketAddr;

// Define your service
pub struct PostgresMcpService {
    // Your service dependencies
}

impl PostgresMcpService {
    pub fn new() -> Self {
        Self {}
    }
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SqlRequest {
    #[property(description = "SQL query to execute")]
    pub sql: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SqlResponse {
    #[property(description = "Query result as JSON")]
    pub result: String,
}

impl ToolDefinition for PostgresMcpService {
    const FUNC_NAME: &'static str = "sql_request";
    const DESCRIPTION: &'static str = "Execute SQL queries";
}

#[async_trait::async_trait]
impl McpToolCall<SqlRequest, SqlResponse> for PostgresMcpService {
    async fn execute_tool_call(
        &self,
        model: SqlRequest,
    ) -> Result<SqlResponse, String> {
        // Execute your SQL query
        let result = execute_query(&model.sql).await?;
        Ok(SqlResponse { result })
    }
}

// Setup function
async fn setup_server() {
    let mut http_server = MyHttpServer::new(SocketAddr::from(([0, 0, 0, 0], 8005)));
    
    // Create middleware
    let mut mcp_middleware = McpMiddleware::new(
        "/postgres",
        "Postgres MCP Server",
        "0.1.0",
        "Execute SQL queries on your database",
    );
    
    // Register tool
    let service = Arc::new(PostgresMcpService::new());
    mcp_middleware.register_tool_call(service);
    
    // Add to server
    let mcp_middleware = Arc::new(mcp_middleware);
    http_server.add_middleware(mcp_middleware);
    
    // Start server
    http_server.start(app_states, logger);
}
```

## API Reference

### `McpMiddleware`

The main middleware struct that handles MCP protocol communication.

#### `new(path, name, version, instructions)`

Creates a new middleware instance.

* `path`: The HTTP path where MCP requests will be handled (e.g., `/mcp`, `/api/mcp`)
* `name`: Server name displayed to clients
* `version`: Server version string
* `instructions`: Instructions for the AI agent using this server

#### `register_tool_call(service)`

Registers a tool call service. The service must implement:

* `McpToolCall<InputData, OutputData>` trait
* `ToolDefinition` trait
* Input and output types must implement `JsonTypeDescription`, `Serialize`, and `DeserializeOwned`

#### `register_tool_call_with_context(service)`

Same as `register_tool_call`, but for tools that need to reach back to
the client during execution (server→client elicitation, etc.). The
service implements [`McpToolCallEx`] instead of `McpToolCall` and
receives a `&ToolCallContext` argument in its execute method. See the
"Server→client elicitation" section above for a worked example.

#### `register_prompt(prompt)`

Registers a prompt service. The service must implement:

* `McpPromptService` trait
* `PromptDefinition` trait
* The `PromptDefinition` trait requires:
  * `PROMPT_NAME`: Unique identifier for the prompt (const)
  * `DESCRIPTION`: Human-readable description (const)
  * `get_argument_descriptions()`: Returns `Vec<PromptArgumentDescription>` with argument metadata

#### `register_resource(service)`

Registers a static resource whose URI is known at compile time. The
service must implement `ResourceDefinition` (provides `RESOURCE_URI`,
`RESOURCE_NAME`, `DESCRIPTION`, `MIME_TYPE` consts plus optional
`get_title` / `get_size` / `get_icons`) and `McpResourceService`
(provides `read_resource`).

#### `register_dynamic_resource(uri, name, description, mime_type, service)` *(async)*

Registers a resource minted at runtime. URI is a `String` chosen by the
caller. Only the service's `McpResourceService` impl is needed —
`ResourceDefinition` is not. Idempotent: re-registering the same URI
overwrites the previous entry. Backed by a `RwLock`, so it's safe to
call after the middleware is wrapped in `Arc` and mounted.

#### `register_dynamic_resource_full(uri, name, description, mime_type, title, size, icons, service)` *(async)*

Same as `register_dynamic_resource` but accepts the optional
`title: Option<String>`, `size: Option<u64>`, and `icons: Vec<ResourceIcon>`
metadata.

#### `unregister_dynamic_resource(uri)` *(async)*

Removes a dynamic resource. Returns `true` if a resource with that URI
was present. Follow up with `notify_resources_changed()` so clients
refresh their resource list.

#### `notify_resource_updated(uri)` *(async)*

Sends `notifications/resources/updated` for `uri` to every live session
that subscribed to it via `resources/subscribe`. Call it whenever the
content behind a resource changes.

#### `with_session_idle_timeout(timeout)`

Builder-style override for the session GC idle timeout (default 30
minutes). Sessions without a live SSE stream that stay untouched longer
than this are dropped by the background sweeper.

### `McpToolCall` Trait

Trait that must be implemented by your tool services:

```rust
#[async_trait::async_trait]
pub trait McpToolCall<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call(&self, model: InputData) -> Result<OutputData, String>;
}
```

### `McpToolCallEx` Trait

Context-aware variant of `McpToolCall`. Implement this when the tool
needs to perform server→client interactions during execution
(elicitation today; sampling/progress in the future). Register the
service with `register_tool_call_with_context`.

```rust
#[async_trait::async_trait]
pub trait McpToolCallEx<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call(
        &self,
        model: InputData,
        ctx: &ToolCallContext,
    ) -> Result<OutputData, String>;
}
```

Note: this trait returns `OutputData` directly — there is no
`ToolCallOutput` wrapper, so inline instructions (the `content[0].text`
channel) are not available on this path. See "Caveat: no inline
instruction on the context-aware path" above.

### `ToolCallContext`

Built by the middleware per tool call and passed into
`McpToolCallEx::execute_tool_call`. Exposes the session id, the
client's `elicitation` capability flag, and the `elicit(...)` method
documented in the "Server→client elicitation" section.

```rust
pub struct ToolCallContext {
    pub session_id: String,
    pub supports_elicitation: bool,
    // ...internal handles
}

impl ToolCallContext {
    pub async fn elicit(
        &self,
        message: &str,
        requested_schema: serde_json::Value,
        timeout: Duration,
    ) -> Result<ElicitationResponse, String>;
}
```

### `ElicitationAction` / `ElicitationResponse`

Returned by `ToolCallContext::elicit`. Per the MCP spec the client
replies with one of three actions; `content` is `Some` only on `Accept`.

```rust
pub enum ElicitationAction { Accept, Decline, Cancel }

pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<serde_json::Value>,
}
```

### `ToolDefinition` Trait

Provides metadata about your tool:

```rust
pub trait ToolDefinition {
    const FUNC_NAME: &'static str;
    const DESCRIPTION: &'static str;
}
```

### `PromptDefinition` Trait

Trait that must be implemented by your prompt services:

```rust
pub trait PromptDefinition {
    const PROMPT_NAME: &'static str;
    const DESCRIPTION: &'static str;
    
    fn get_argument_descriptions() -> Vec<PromptArgumentDescription>;
}
```

### `PromptArgumentDescription` Struct

Represents a prompt argument description:

```rust
pub struct PromptArgumentDescription {
    pub name: String,
    pub description: String,
    pub required: bool,
}
```

### `McpPromptService` Trait

Trait that must be implemented by your prompt services:

```rust
#[async_trait::async_trait]
pub trait McpPromptService {
    async fn execute_prompt(
        &self,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptExecutionResult, String>;
}
```

## MCP Protocol Support

The middleware implements the MCP Streamable HTTP transport (protocol revisions `2025-03-26` / `2025-06-18` / `2025-11-25`, negotiated at `initialize`) and handles the following protocol methods:

### Core Protocol Methods

* **`initialize`**: Initializes a new MCP session and returns server capabilities
  - Negotiates the protocol version (supported revisions echoed, unknown → latest supported)
  - Declares `tools` / `prompts` capabilities when registered; the `resources` capability (with `subscribe` and `listChanged`) is advertised **always**, because dynamic resources may be registered at any moment after initialize
  - Returns server information and creates a new session with a unique session ID
  - Accepted with or without a stale session header — re-initialization always works

* **`tools/list`**: Returns a list of available tools with their JSON schemas
  - Includes input and output schemas for each tool
  - Generated automatically from your Rust types using `ApplyJsonSchema`

* **`tools/call`**: Executes a tool call with the provided arguments
  - Validates input against the tool's schema
  - Executes your service implementation
  - Returns structured results or errors

* **`prompts/list`**: Returns a list of available prompts with their arguments
  - Shows prompt names, descriptions, and argument definitions
  - Includes required/optional status for each argument

* **`prompts/get`**: Retrieves a prompt with variable substitution
  - Executes the prompt template with provided arguments
  - Returns formatted prompt messages ready for AI consumption

* **`resources/list`**: Returns available resources with metadata
  - Supports pagination via cursor-based navigation
  - Includes resource URI, name, description, MIME type, and optional metadata (title, size, icons)

* **`resources/read`**: Reads resource contents
  - Returns text or binary content based on resource type
  - Supports multiple content blocks per resource

* **`resources/templates/list`**: Returns an empty `resourceTemplates` list (URI templates are not supported, but clients that call this unconditionally get a valid response)

* **`resources/subscribe`** / **`resources/unsubscribe`**: Per-session subscriptions to resource changes
  - Subscribe validates the URI (unknown URI → `-32002 Resource not found`) and answers with an empty result, per spec
  - Push updates to subscribers from your code via `McpMiddleware::notify_resource_updated(uri)` — subscribed sessions with a live SSE stream receive `notifications/resources/updated`

* **`ping`**: Health check endpoint for connection testing

* **`notifications/initialized`**: Handles client initialization acknowledgment

* **Any other `notifications/*`** (e.g. `notifications/cancelled`): accepted with HTTP `202` and ignored, per the Streamable HTTP transport

* **Unknown request methods**: answered with a standard JSON-RPC error `-32601 Method not found` instead of breaking the session

* **`elicitation/create`** *(server→client)*: Sent by the server to ask the connected client to prompt the user for input. Carries a message and a JSON schema describing the expected reply. Triggered from tool code via `ToolCallContext::elicit(...)`. Requires the client to advertise `capabilities.elicitation` at `initialize`. The client responds over the regular POST endpoint with the matching request id, and the middleware wakes the parked tool call. See the "Server→client elicitation" section for the full flow.

### Protocol Features

- **JSON-RPC 2.0**: All requests/responses follow JSON-RPC 2.0 format; request `id` may be a number **or a string** and is echoed back exactly as received
- **Protocol version negotiation**: a supported `protocolVersion` is echoed; an unknown one is answered with the latest supported revision
- **Server-Sent Events (SSE)**: Streaming responses for real-time updates
- **Long tool calls survive proxies**: the `tools/call` response stream opens immediately and emits `: keepalive` SSE comments every 15s while the tool runs (essential for elicitation, where a human may think for minutes). If the client disconnects mid-call, the tool future is dropped (the call is cancelled)
- **Session Management**: Secure session-based authentication via `mcp-session-id` header
- **Type Safety**: Automatic JSON schema generation from Rust types
- **Error Handling**: Standardized JSON-RPC error objects (`error: {code, message}`) with MCP codes: `-32700` parse error, `-32601` method not found, `-32602` invalid params / unknown tool / unknown prompt, `-32002` resource not found, `-32603` internal error

### HTTP status semantics

| Situation | Status |
|---|---|
| Request/response handled | `200` (SSE stream) |
| Notification or client JSON-RPC response accepted | `202` |
| Missing `mcp-session-id` header (non-initialize) | `400` |
| Unparsable JSON-RPC body | `400` + JSON-RPC `-32700` body |
| Unknown / expired session (POST, GET, DELETE) | `404` — per spec the client re-initializes |
| Session deleted via DELETE | `204` |

`initialize` is accepted with or without a (possibly stale) session header and always mints a fresh session.

## Session Management

Sessions are automatically managed by the middleware:

* Each `initialize` request creates a new session with a unique session ID
* Session IDs are returned in the `mcp-session-id` HTTP header
* Subsequent requests must include the session ID in the `mcp-session-id` header
* GET requests to the MCP path establish Server-Sent Events (SSE) streams for notifications
* Sessions that have **no live SSE stream** and stay idle longer than the configured timeout are garbage-collected by a background sweeper (sweep interval 60s). The default idle timeout is 30 minutes; override it with `McpMiddleware::with_session_idle_timeout(Duration)`. Sessions with an open GET stream are never collected — a dead stream is detected within a couple of keepalive intervals and only then does the idle clock apply
* `DELETE` with the session header terminates the session explicitly (`204`)

## Type Safety

The middleware leverages `my-ai-agent`'s `ApplyJsonSchema` macro to automatically generate JSON schemas for your input and output types. This ensures type safety and automatic schema generation for MCP tool definitions. Use the `#[property(description = "...")]` attribute to document your fields:

```rust
#[derive(ApplyJsonSchema, Serialize, Deserialize)]
pub struct MyRequest {
    #[property(description: "A description of this field")]
    pub field: String,
}
```

The generated schemas are automatically used when clients call `tools/list` to discover available tools. Similarly, registered prompts are exposed when clients call `prompts/list`.

## Error Handling

Tool execution errors should be returned as `Err(String)` from `execute_tool_call`. The middleware reports them **in-band** per the MCP spec — the `tools/call` response carries `isError: true` with the message in `content[0].text` — so the model can see and react to the failure.

Protocol-level problems are reported as JSON-RPC error objects instead:

* unknown tool / unknown prompt → `-32602 Invalid params`
* unknown resource URI on `resources/read` / `resources/subscribe` → `-32002 Resource not found`
* unknown method → `-32601 Method not found`
* unparsable request body → HTTP `400` with a `-32700 Parse error` body
* resource read / prompt execution failure → `-32603 Internal error`

## Best Practices

### Naming Conventions

* **Tool files**: `{snake_case_name}_tool_call.rs`
* **Prompt files**: `{snake_case_name}_prompt.rs` or add to existing prompt files
* **Handler structs**: `{PascalCaseName}Handler`
* **Input/Output structs**: `{PascalCaseName}InputData` / `{PascalCaseName}Response`

### Error Handling

* Always return descriptive error messages
* Use `Result<T, String>` - the String will be sent to the client
* Handle errors gracefully and provide context

### Documentation

* Use `#[property(description = "...")]` for all fields in input/output structs
* Write clear `DESCRIPTION` constants for tools and prompts
* Document complex logic in comments

### Project Structure

When building an MCP server using this middleware, organize your code as follows:

```
src/
├── lib.rs or main.rs
├── mcp/                          # MCP tool calls and prompts
│   ├── mod.rs                    # Export all MCP components
│   ├── {tool_name}_tool_call.rs  # Individual tool implementations
│   └── {prompt_name}_prompt.rs   # Individual prompt implementations
└── http/
    └── start_up.rs               # Register tools and prompts here
```

### Registration Order

In your startup code, register components in this order:

1. Create `McpMiddleware` instance
2. Register all tool calls using `register_tool_call()`
3. Register all prompts using `register_prompt()`
4. Register all resources using `register_resource()`
5. Add middleware to HTTP server

```rust
let mut mcp = McpMiddleware::new(
    "/mcp",
    "My MCP Server",
    "0.1.0",
    "Server description",
);

// Register tools
mcp.register_tool_call(Arc::new(Tool1Handler::new()));
mcp.register_tool_call(Arc::new(Tool2Handler::new()));

// Register prompts
mcp.register_prompt(Arc::new(Prompt1Handler));
mcp.register_prompt(Arc::new(Prompt2Handler));

// Register static resources
mcp.register_resource(Arc::new(Resource1Handler));

// Add to HTTP server
let mcp = Arc::new(mcp);
http_server.add_middleware(mcp.clone());

// Dynamic resources can be registered any time after this, e.g. from
// a tool handler that just produced an artifact:
// mcp.register_dynamic_resource(uri, name, desc, mime, svc).await;
// mcp.notify_resources_changed().await;
```

## Use Cases

This middleware can be used to build MCP servers for various purposes:

* **Database Access**: Expose database operations (SQL queries, schema inspection, etc.)
* **File System Operations**: Provide file and directory management capabilities
* **API Integrations**: Wrap external APIs and services as MCP tools
* **Development Tools**: Expose build, test, and deployment operations
* **Custom Business Logic**: Implement domain-specific tools for your application
* **Prompt Templates**: Register reusable prompt templates that AI agents can use with variable substitution

The Postgres example above demonstrates one such use case. You can adapt the same pattern to implement tools for any functionality you need. Prompts are useful for providing pre-configured prompt templates that clients can use with different variable values.

## Dependencies

* `my-http-server`: HTTP server framework
* `my-ai-agent`: AI agent utilities and JSON schema generation
* `tokio`: Async runtime
* `serde` / `serde_json`: Serialization
* `async-trait`: Async trait support

## License

[Add your license here]

## Contributing

[Add contribution guidelines here]

