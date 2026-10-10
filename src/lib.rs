mod mcp_middleware;
pub use mcp_middleware::*;

/// Re-exported so a host can implement [`McpConnectionInfo`] against
/// exactly the `HttpContext` this crate was built with.
pub use my_http_server;

/// JSON schema generation, from [json-schema](https://github.com/my-jet-tools/json-schema).
/// Re-exported together with an `ApplyJsonSchema` derive which refers to it through this
/// crate, so a crate which uses the middleware needs no `json-schema` dependency of its own.
pub use json_schema;
pub use json_schema::my_json;
pub use mcp_server_middleware_macros as macros;
pub use mcp_server_middleware_macros::ApplyJsonSchema;

// The `ApplyJsonSchema` derive generates `::mcp_server_middleware::json_schema` paths;
// this lets them resolve inside this crate too.
extern crate self as mcp_server_middleware;
