# MCP Server Middleware

A Rust middleware library for implementing Model Context Protocol (MCP) servers. This middleware handles MCP protocol communication, session management, and tool call execution, making it easy to build MCP-compatible servers for any use case.

The middleware provides a flexible, trait-based architecture that allows you to implement custom tool calls for any domain - whether it's database access, file operations, API integrations, or any other functionality you want to expose through the MCP protocol.

## About Model Context Protocol (MCP)

The Model Context Protocol (MCP) is a standardized protocol (specification version 2025-11-25) that enables AI applications to securely access external data sources and tools. MCP provides a unified interface for AI agents to interact with external systems, databases, APIs, and services.

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
- `ResourceDefinition` & `McpResourceService`: Traits for resource management

**Type Safety**:
- Automatic JSON schema generation from Rust types using `ApplyJsonSchema` macro
- Compile-time type checking ensures schemas match implementation
- Support for dynamic enum values based on runtime data

**Protocol Compliance**:
- Full implementation of MCP protocol specification (2025-11-25)
- All required protocol methods (`initialize`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, `resources/list`, `resources/read`, `ping`)
- Proper JSON-RPC 2.0 formatting
- SSE streaming support
- Session management with secure session IDs

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
mcp_middleware.register_tool_call(service).await;
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
mcp_middleware.register_prompt(prompt_service).await;
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
    async fn read_resource(&self, uri: &str) -> Result<ResourceReadResult, String> {
        // Read your resource content here
        let content = "Resource content here".to_string();
        
        Ok(ResourceReadResult {
            contents: vec![ResourceContent {
                uri: uri.to_string(),
                mime_type: "text/plain".to_string(),
                text: Some(content),
                blob: None,
            }],
        })
    }
}

// Register the resource
let resource_service = Arc::new(MyResourceService);
mcp_middleware.register_resource(resource_service).await;
```

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
mcp_middleware.register_tool_call(Arc::new(MyToolHandler::new())).await;
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
mcp_middleware.register_prompt(Arc::new(MyPromptHandler)).await;
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

* `McpToolCall<InputData, OutputData>` trait
* `ToolDefinition` trait
* Input and output types must implement `JsonTypeDescription`, `Serialize`, and `DeserializeOwned`

#### `register_prompt(prompt)`

Registers a prompt service. The service must implement:

* `McpPromptService` trait
* `PromptDefinition` trait
* The `PromptDefinition` trait requires:
  * `PROMPT_NAME`: Unique identifier for the prompt (const)
  * `DESCRIPTION`: Human-readable description (const)
  * `get_argument_descriptions()`: Returns `Vec<PromptArgumentDescription>` with argument metadata

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

The middleware fully implements the MCP protocol specification (2025-11-25) and handles the following protocol methods:

### Core Protocol Methods

* **`initialize`**: Initializes a new MCP session and returns server capabilities
  - Declares support for tools, prompts, and resources
  - Returns protocol version and server information
  - Creates a new session with unique session ID

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

* **`resources/subscribe`**: Subscribes to resource changes
  - Returns the initial version of the resource
  - Currently returns the first version only (update notifications not yet implemented)

* **`ping`**: Health check endpoint for connection testing

* **`notifications/initialized`**: Handles client initialization acknowledgment

### Protocol Features

- **JSON-RPC 2.0**: All requests/responses follow JSON-RPC 2.0 format
- **Server-Sent Events (SSE)**: Streaming responses for real-time updates
- **Session Management**: Secure session-based authentication via `mcp-session-id` header
- **Type Safety**: Automatic JSON schema generation from Rust types
- **Error Handling**: Standardized error codes and messages per MCP specification

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
mcp.register_tool_call(Arc::new(Tool1Handler::new())).await;
mcp.register_tool_call(Arc::new(Tool2Handler::new())).await;

// Register prompts
mcp.register_prompt(Arc::new(Prompt1Handler)).await;
mcp.register_prompt(Arc::new(Prompt2Handler)).await;

// Register resources
mcp.register_resource(Arc::new(Resource1Handler)).await;

// Add to HTTP server
let mcp = Arc::new(mcp);
http_server.add_middleware(mcp);
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

