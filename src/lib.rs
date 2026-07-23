mod mcp_middleware;
pub use mcp_middleware::*;

/// Re-exported so a host can implement [`McpConnectionInfo`] against
/// exactly the `HttpContext` this crate was built with.
pub use my_http_server;

pub use my_ai_agent::ToolDefinition;
pub use my_ai_agent::json_schema;
pub use my_ai_agent::macros;
pub use my_ai_agent::macros::ApplyJsonSchema;
pub use my_ai_agent::my_json;
