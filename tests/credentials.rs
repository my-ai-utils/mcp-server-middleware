//! A tool sees who the request came from.
//!
//! The middleware does not authenticate anybody: a middleware registered in
//! front of it does, and leaves its verdict in `HttpContext::credentials` —
//! my-http-server's own slot for it. What is tested here is the hand-over:
//! `ToolCallContext::credentials_id` is that verdict, read from the very
//! request that carried the call. An `HttpContext` can only be produced by
//! hyper, so this drives a real `MyHttpServer` over a loopback socket.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mcp_server_middleware::my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpRequestHeaders, HttpServerMiddleware,
    MyHttpServer, RequestClaim, RequestCredentials, async_trait,
};
use mcp_server_middleware::{
    ApplyJsonSchema, McpConnectionInfo, McpMiddleware, McpSession, McpToolCallEx,
    ToolCallContext, ToolDefinition,
};
use rust_extensions::{ApplicationStates, Logger};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INITIALIZE_BODY: &str = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;

const WHOAMI_BODY: &str =
    r#"{"jsonrpc":"2.0","method":"tools/call","id":2,"params":{"name":"whoami","arguments":{}}}"#;

/// Stands in for whatever a host authenticates by — an API key, a session
/// cookie: here it is simply the name in `x-test-user`.
struct HeaderCredentials(String);

impl RequestCredentials for HeaderCredentials {
    fn get_id(&self) -> &str {
        self.0.as_str()
    }

    fn get_claims<'s>(&'s self) -> Option<Vec<RequestClaim<'s>>> {
        None
    }
}

struct HeaderAuthentication;

#[async_trait::async_trait]
impl HttpServerMiddleware for HeaderAuthentication {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        let user = ctx
            .request
            .get_headers()
            .try_get_case_insensitive("x-test-user")
            .and_then(|value| value.as_str().ok().map(|value| value.to_string()));

        if let Some(user) = user {
            ctx.credentials = Some(Box::new(HeaderCredentials(user)));
        }

        // Never answers: it only says who is asking, and the request goes on.
        None
    }
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
struct WhoAmIInput {
    #[property(description = "Ignored")]
    note: Option<String>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
struct WhoAmIOutput {
    #[property(description = "Who the request came from")]
    caller: Option<String>,
}

struct WhoAmI;

impl ToolDefinition for WhoAmI {
    const FUNC_NAME: &'static str = "whoami";
    const DESCRIPTION: &'static str = "Says who the request came from";
}

#[async_trait::async_trait]
impl McpToolCallEx<WhoAmIInput, WhoAmIOutput> for WhoAmI {
    async fn execute_tool_call(
        &self,
        _model: WhoAmIInput,
        ctx: &ToolCallContext,
    ) -> Result<WhoAmIOutput, String> {
        Ok(WhoAmIOutput {
            caller: ctx.credentials_id.clone(),
        })
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

/// A host that listens to sessions coming and going. Registering one moves
/// a POST onto the other of the middleware's two paths — the one that copies
/// the body so the whole `HttpContext` can be handed on — and the caller has
/// to arrive along both.
struct Listener;

#[async_trait::async_trait]
impl McpConnectionInfo for Listener {
    async fn on_connected(&self, _session: &McpSession, _ctx: &mut HttpContext) {}

    async fn on_disconnected(&self, _session: &McpSession) {}
}

/// The authentication in front, the MCP middleware behind it — the order a
/// host registers them in.
async fn start_server(listening: bool) -> SocketAddr {
    let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
    mcp.register_tool_call_with_context(Arc::new(WhoAmI));

    if listening {
        mcp.register_connection_info(Arc::new(Listener));
    }

    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };

    let mut server = MyHttpServer::new(addr);
    server.add_middleware(Arc::new(HeaderAuthentication));
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

fn post(body: &str, session_id: Option<&str>, user: Option<&str>) -> String {
    let session_header = match session_id {
        Some(session_id) => format!("mcp-session-id: {}\r\n", session_id),
        None => String::new(),
    };

    let user_header = match user {
        Some(user) => format!("x-test-user: {}\r\n", user),
        None => String::new(),
    };

    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{}{}Content-Length: {}\r\n\r\n{}",
        session_header,
        user_header,
        body.len(),
        body
    )
}

/// Writes a raw request and reads the response until `needle` shows up in
/// it. The body is a stream on a kept-alive connection, so there is no end
/// to wait for — only the thing the test is looking for.
async fn send_and_read_until(addr: SocketAddr, request: String, needle: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];

    loop {
        let text = String::from_utf8_lossy(&response).to_string();

        if text.contains(needle) {
            return text;
        }

        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("no {needle} in the response: {text}"))
            .expect("failed to read the response");

        if read == 0 {
            panic!("the connection closed with no {needle} in the response: {text}");
        }

        response.extend_from_slice(&buf[..read]);
    }
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

async fn whoami_says(addr: SocketAddr, user: &str) {
    let head = send_and_read_until(addr, post(INITIALIZE_BODY, None, None), "\r\n\r\n").await;
    let session_id = session_header(&head).expect("initialize must return mcp-session-id");

    send_and_read_until(
        addr,
        post(WHOAMI_BODY, Some(&session_id), Some(user)),
        format!(r#""caller":"{}""#, user).as_str(),
    )
    .await;
}

#[tokio::test]
async fn a_tool_sees_the_credentials_of_the_request_that_carried_it() {
    whoami_says(start_server(false).await, "alice").await;
}

#[tokio::test]
async fn a_tool_sees_them_when_the_host_listens_to_sessions_too() {
    whoami_says(start_server(true).await, "alice").await;
}

#[tokio::test]
async fn credentials_are_read_per_request_not_per_session() {
    let addr = start_server(false).await;

    let head = send_and_read_until(addr, post(INITIALIZE_BODY, None, Some("alice")), "\r\n\r\n").await;
    let session_id = session_header(&head).expect("initialize must return mcp-session-id");

    // The same session, somebody else speaking in it: the call is theirs.
    let answer = send_and_read_until(
        addr,
        post(WHOAMI_BODY, Some(&session_id), Some("bob")),
        r#""caller":"bob""#,
    )
    .await;
    assert!(!answer.contains(r#""caller":"alice""#), "{answer}");

    // And nobody at all: nothing upstream vouched for this request.
    send_and_read_until(
        addr,
        post(WHOAMI_BODY, Some(&session_id), None),
        r#""caller":null"#,
    )
    .await;
}
