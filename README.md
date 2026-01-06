# MCP Server Middleware

A Rust middleware library for implementing Model Context Protocol (MCP) servers. This middleware handles MCP protocol communication, session management, and tool call execution, making it easy to build MCP-compatible servers for any use case.

The middleware provides a flexible, trait-based architecture that allows you to implement custom tool calls for any domain - whether it's database access, file operations, API integrations, or any other functionality you want to expose through the MCP protocol.

## Features

* **MCP Protocol Support**: Full implementation of MCP protocol including initialization, tool calls, prompts, and notifications
* **Session Management**: Automatic session creation and management with session-based authentication
* **Tool Call Framework**: Easy-to-use trait-based system for implementing custom tool calls
* **Prompt Support**: Register and expose prompts that MCP clients can discover and use
* **HTTP Integration**: Seamless integration with `my-http-server` as middleware
* **Type-Safe Tool Definitions**: Leverages `my-ai-agent` for type-safe JSON schema generation
* **Dynamic Enumeration**: Support for dynamically generated enum values based on runtime data

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

Create a service that implements the `McpService` trait:

```rust
use mcp_server_middleware::{McpService, ToolDefinition};
use my_ai_agent::{macros::ApplyJsonSchema, json_schema::*};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;

// Define your input and output types with JSON schema
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolRequest {
    #[property(description: "Input parameter description")]
    pub input_field: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct MyToolResponse {
    #[property(description: "Output parameter description")]
    pub output_field: String,
}

// Implement ToolDefinition to provide metadata
impl ToolDefinition for MyMcpService {
    const FUNC_NAME: &'static str = "my_tool";
    const DESCRIPTION: &'static str = "Description of what this tool does";
}

// Implement McpService to handle tool execution
#[async_trait]
impl McpService<MyToolRequest, MyToolResponse> for MyMcpService {
    async fn execute_tool_call(
        &self,
        request: MyToolRequest,
    ) -> Result<MyToolResponse, String> {
        // Your implementation here
        let result = self.process_request(request.input_field).await?;
        
        Ok(MyToolResponse {
            output_field: result,
        })
    }
}
```

### 3. Register Tool Calls

Register your service with the middleware:

```rust
let service = Arc::new(MyMcpService::new(app_context));
mcp_middleware.register_tool_call(service).await;
```

### 4. Register Prompts (Optional)

You can also register prompts that MCP clients can discover and use:

```rust
use mcp_server_middleware::PromptDefinition;

// Create a prompt with arguments
let prompt = PromptDefinition::new(
    "example_prompt".to_string(),
    "An example prompt that demonstrates prompt functionality".to_string()
)
.with_argument(
    "variable_name".to_string(),
    "Description of what this variable represents".to_string(),
    true  // required
)
.with_argument(
    "optional_param".to_string(),
    "An optional parameter".to_string(),
    false  // optional
);

mcp_middleware.register_prompt(prompt);
```

You can also build prompts incrementally:

```rust
let mut prompt = PromptDefinition::new(
    "my_prompt".to_string(),
    "My prompt description".to_string()
);
prompt.add_argument("param1".to_string(), "First parameter".to_string(), true);
prompt.add_argument("param2".to_string(), "Second parameter".to_string(), false);

mcp_middleware.register_prompt(prompt);
```

### 5. Integrate with HTTP Server

Add the middleware to your HTTP server:

```rust
use my_http_server::MyHttpServer;
use std::net::SocketAddr;

let mut http_server = MyHttpServer::new(SocketAddr::from(([0, 0, 0, 0], 8005)));
let mcp_middleware = Arc::new(mcp_middleware);
http_server.add_middleware(mcp_middleware);
http_server.start(app_states, logger);
```

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

## Complete Example: Postgres MCP Server

The following example demonstrates a real-world implementation - a Postgres MCP server that allows AI agents to execute SQL queries. This serves as a concrete reference for building your own MCP servers:

```rust
use std::sync::Arc;
use mcp_server_middleware::{McpMiddleware, McpService, ToolDefinition};
use my_http_server::MyHttpServer;
use my_ai_agent::{macros::ApplyJsonSchema, json_schema::*};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;

// Define your service
pub struct PostgresMcpService {
    // Your service dependencies
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SqlRequest {
    #[property(description: "SQL query to execute")]
    pub sql: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SqlResponse {
    #[property(description: "Query result as JSON")]
    pub result: String,
}

impl ToolDefinition for PostgresMcpService {
    const FUNC_NAME: &'static str = "sql_request";
    const DESCRIPTION: &'static str = "Execute SQL queries";
}

#[async_trait]
impl McpService<SqlRequest, SqlResponse> for PostgresMcpService {
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
    mcp_middleware.register_tool_call(service).await;
    
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

* `McpService<InputData, OutputData>` trait
* `ToolDefinition` trait
* Input and output types must implement `JsonTypeDescription`, `Serialize`, and `DeserializeOwned`

#### `register_prompt(prompt)`

Registers a prompt definition. The prompt must be a `PromptDefinition` instance with:
* `name`: Unique identifier for the prompt
* `description`: Human-readable description of what the prompt does
* `arguments`: Optional list of `PromptArgument` objects, each with:
  * `name`: Argument identifier
  * `description`: What the argument represents
  * `required`: Whether the argument is required (boolean)

### `McpService` Trait

Trait that must be implemented by your tool services:

```rust
#[async_trait]
pub trait McpService<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call(&self, model: InputData) -> Result<OutputData, String>;
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

### `PromptDefinition` Struct

Represents a prompt that can be registered with the middleware:

```rust
pub struct PromptDefinition {
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}
```

Create prompts using the builder pattern:

```rust
let prompt = PromptDefinition::new(
    "prompt_name".to_string(),
    "Description".to_string()
)
.with_argument("arg1".to_string(), "Arg description".to_string(), true);
```

## MCP Protocol Support

The middleware handles the following MCP protocol methods:

* **`initialize`**: Initializes a new MCP session and returns server capabilities
* **`tools/list`**: Returns a list of available tools with their schemas
* **`tools/call`**: Executes a tool call with the provided arguments
* **`prompts/list`**: Returns a list of available prompts with their arguments
* **`ping`**: Health check endpoint
* **`resources/list`**: Returns available resources (currently returns empty)
* **`notifications/initialized`**: Handles initialization notifications

## Session Management

Sessions are automatically managed by the middleware:

* Each `initialize` request creates a new session with a unique session ID
* Session IDs are returned in the `mcp-session-id` HTTP header
* Subsequent requests must include the session ID in the `mcp-session-id` header
* GET requests to the MCP path establish Server-Sent Events (SSE) streams for notifications

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

Tool execution errors should be returned as `Err(String)` from `execute_tool_call`. The middleware will format these appropriately in the MCP response format, ensuring clients receive properly structured error information.

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

