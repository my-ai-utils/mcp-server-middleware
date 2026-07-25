//! The error hook over a real HTTP server.
//!
//! The interesting failure — a `tools/call` whose arguments do not match
//! the tool's `inputSchema` — is refused before the tool ever runs, so a
//! host wrapping its own tool implementations can not see it. This test
//! drives an actual `MyHttpServer` over a loopback socket and asserts the
//! host hook is handed the tool name and the raw arguments the client
//! sent.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mcp_server_middleware::my_http_server::{MyHttpServer, async_trait};
use mcp_server_middleware::*;
use parking_lot::Mutex;
use rust_extensions::{ApplicationStates, Logger};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INITIALIZE_BODY: &str = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;

/// The arguments the model actually sent in the failure this hook was
/// written for: `query` invented for a field the schema calls `pattern`.
const BAD_ARGUMENTS: &str = r#"{"project":"mt-risks","query":"Account groups"}"#;

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SearchInput {
    #[property(description = "What to search for")]
    pub pattern: String,
    #[property(description = "Which project to search in")]
    pub project: Option<String>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SearchOutput {
    #[property(description = "What was found")]
    pub found: String,
}

pub struct SearchTool;

impl ToolDefinition for SearchTool {
    const FUNC_NAME: &'static str = "search";
    const DESCRIPTION: &'static str = "Searches by pattern";
}

#[async_trait::async_trait]
impl McpToolCall<SearchInput, SearchOutput> for SearchTool {
    async fn execute_tool_call(&self, model: SearchInput) -> Result<SearchOutput, String> {
        Ok(SearchOutput {
            found: model.pattern,
        })
    }
}

/// Owned copy of an event — [`McpMiddlewareError`] borrows, so a host
/// that wants to keep an event copies out what it needs. This is what a
/// real host's `ActivityLog` push looks like.
#[derive(Debug, Clone, PartialEq)]
enum Recorded {
    ToolInputDeserialization {
        session_id: String,
        tool_name: String,
        arguments: String,
        error: String,
    },
    ToolExecution {
        session_id: String,
        tool_name: String,
        arguments: String,
    },
    ToolNotFound {
        session_id: String,
        tool_name: String,
        arguments: String,
    },
    SessionRejected {
        session_id: Option<String>,
        http_method: String,
        error: String,
    },
    Other(String),
}

#[derive(Default)]
struct Recorder {
    errors: Mutex<Vec<Recorded>>,
}

impl Recorder {
    fn errors(&self) -> Vec<Recorded> {
        self.errors.lock().clone()
    }
}

#[async_trait::async_trait]
impl McpMiddlewareErrorHook for Recorder {
    async fn on_error(&self, err: McpMiddlewareError<'_>) {
        let recorded = match err {
            McpMiddlewareError::ToolInputDeserialization {
                session_id,
                tool_name,
                arguments,
                error,
            } => Recorded::ToolInputDeserialization {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.to_string(),
                error: error.to_string(),
            },
            McpMiddlewareError::ToolExecution {
                session_id,
                tool_name,
                arguments,
                ..
            } => Recorded::ToolExecution {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.to_string(),
            },
            McpMiddlewareError::ToolNotFound {
                session_id,
                tool_name,
                arguments,
            } => Recorded::ToolNotFound {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.to_string(),
            },
            McpMiddlewareError::SessionRejected {
                session_id,
                http_method,
                error,
            } => Recorded::SessionRejected {
                session_id: session_id.map(|id| id.to_string()),
                http_method: http_method.to_string(),
                error: error.to_string(),
            },
            other => Recorded::Other(other.to_string()),
        };

        self.errors.lock().push(recorded);
    }
}

struct TestAppStates;

impl ApplicationStates for TestAppStates {
    fn is_initialized(&self) -> bool {
        true
    }

    fn is_shutting_down(&self) -> bool {
        false
    }
}

struct TestLogger;

impl Logger for TestLogger {
    fn write_info(&self, _process: String, _message: String, _ctx: Option<HashMap<String, String>>) {
    }

    fn write_warning(
        &self,
        _process: String,
        _message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
    }

    fn write_error(
        &self,
        _process: String,
        _message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
    }

    fn write_fatal_error(
        &self,
        _process: String,
        _message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
    }

    fn write_debug_info(
        &self,
        _process: String,
        _message: String,
        _ctx: Option<HashMap<String, String>>,
    ) {
    }
}

async fn start_server(recorder: Arc<Recorder>) -> SocketAddr {
    let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
    mcp.register_tool_call(Arc::new(SearchTool));
    mcp.register_error_hook(recorder);

    // Take a port from the OS, then hand it over to the http server.
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };

    let mut server = MyHttpServer::new(addr);
    server.add_middleware(Arc::new(mcp));
    server.start(Arc::new(TestAppStates), Arc::new(TestLogger));

    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!("http server did not start listening on {}", addr);
}

/// Writes a raw request and reads until `stop_at` shows up in the
/// response (or the head is complete and `stop_at` is `None`). Responses
/// are streamed on a kept-alive connection, so there is no EOF to wait
/// for.
async fn send_raw(addr: SocketAddr, request: String, stop_at: Option<&str>) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];

    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timed out reading the response")
            .expect("failed to read the response");

        if read == 0 {
            break;
        }

        response.extend_from_slice(&buf[..read]);

        match stop_at {
            Some(needle) => {
                if String::from_utf8_lossy(&response).contains(needle) {
                    break;
                }
            }
            None => {
                if response.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }

    String::from_utf8_lossy(&response).to_string()
}

fn status_code(head: &str) -> u16 {
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("no status code in the response")
}

fn session_header(head: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("mcp-session-id") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn post(body: &str, session_id: Option<&str>) -> String {
    let session_header = match session_id {
        Some(session_id) => format!("mcp-session-id: {}\r\n", session_id),
        None => String::new(),
    };

    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\n\r\n{}",
        session_header,
        body.len(),
        body
    )
}

async fn initialize(addr: SocketAddr) -> String {
    let head = send_raw(addr, post(INITIALIZE_BODY, None), None).await;
    assert_eq!(status_code(&head), 200);
    session_header(&head).expect("initialize must return mcp-session-id")
}

#[tokio::test]
async fn arguments_that_do_not_match_the_schema_reach_the_error_hook() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;
    let session_id = initialize(addr).await;

    let call = format!(
        r#"{{"jsonrpc":"2.0","method":"tools/call","id":2,"params":{{"name":"search","arguments":{}}}}}"#,
        BAD_ARGUMENTS
    );
    let response = send_raw(addr, post(&call, Some(&session_id)), Some("isError")).await;

    assert_eq!(status_code(&response), 200);
    // The wire answer is what it always was — an in-band tool error.
    assert!(response.contains(r#""isError":true"#), "{}", response);

    let errors = recorder.errors();
    assert_eq!(errors.len(), 2, "got {:?}", errors);

    assert_eq!(
        errors[0],
        Recorded::ToolInputDeserialization {
            session_id: session_id.clone(),
            tool_name: "search".to_string(),
            arguments: BAD_ARGUMENTS.to_string(),
            error: "missing field `pattern` at line 1 column 47".to_string(),
        }
    );

    // The same call also failed as a tool call, exactly as it printed two
    // lines before the hook existed.
    assert_eq!(
        errors[1],
        Recorded::ToolExecution {
            session_id,
            tool_name: "search".to_string(),
            arguments: BAD_ARGUMENTS.to_string(),
        }
    );
}

/// The refusal sent back names the schema, so the model can correct
/// `query` to `pattern` on its next turn.
#[tokio::test]
async fn the_refusal_sent_back_carries_the_input_schema() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder).await;
    let session_id = initialize(addr).await;

    let call = format!(
        r#"{{"jsonrpc":"2.0","method":"tools/call","id":2,"params":{{"name":"search","arguments":{}}}}}"#,
        BAD_ARGUMENTS
    );
    let response = send_raw(addr, post(&call, Some(&session_id)), Some("isError")).await;

    assert!(response.contains("missing field"), "{}", response);
    assert!(response.contains("Expected schema"), "{}", response);
    assert!(response.contains("pattern"), "{}", response);
}

/// `GET` opens the SSE notification stream and `DELETE` closes a session
/// — both are refused on a bad `mcp-session-id`, and neither goes through
/// the POST path, so only a real server exercises them.
#[tokio::test]
async fn session_level_refusals_reach_the_error_hook_on_every_verb() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;

    // GET with no session header at all → 400.
    let head = send_raw(
        addr,
        "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
        None,
    )
    .await;
    assert_eq!(status_code(&head), 400);

    // GET naming a session the server does not have → 404.
    let head = send_raw(
        addr,
        "GET /mcp HTTP/1.1\r\nHost: localhost\r\nmcp-session-id: ghost\r\n\r\n".to_string(),
        None,
    )
    .await;
    assert_eq!(status_code(&head), 404);

    // DELETE of a session that is already gone → 404.
    let head = send_raw(
        addr,
        "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\nmcp-session-id: ghost\r\nContent-Length: 0\r\n\r\n"
            .to_string(),
        None,
    )
    .await;
    assert_eq!(status_code(&head), 404);

    // POST without the header → 400.
    let head = send_raw(
        addr,
        post(r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#, None),
        None,
    )
    .await;
    assert_eq!(status_code(&head), 400);

    assert_eq!(
        recorder.errors(),
        vec![
            Recorded::SessionRejected {
                session_id: None,
                http_method: "GET".to_string(),
                error: "Missing mcp-session-id header".to_string(),
            },
            Recorded::SessionRejected {
                session_id: Some("ghost".to_string()),
                http_method: "GET".to_string(),
                error: "Unknown MCP session".to_string(),
            },
            Recorded::SessionRejected {
                session_id: Some("ghost".to_string()),
                http_method: "DELETE".to_string(),
                error: "Unknown MCP session".to_string(),
            },
            Recorded::SessionRejected {
                session_id: None,
                http_method: "POST".to_string(),
                error: "Missing mcp-session-id header".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn an_unregistered_tool_reaches_the_error_hook() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;
    let session_id = initialize(addr).await;

    let call = r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"grep","arguments":{"pattern":"x"}}}"#;
    let response = send_raw(addr, post(call, Some(&session_id)), Some("Unknown tool")).await;
    assert!(response.contains("Unknown tool: grep"), "{}", response);

    assert_eq!(
        recorder.errors(),
        vec![Recorded::ToolNotFound {
            session_id,
            tool_name: "grep".to_string(),
            arguments: r#"{"pattern":"x"}"#.to_string(),
        }]
    );
}
