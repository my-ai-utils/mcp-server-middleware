//! Session lifecycle events over a real HTTP server.
//!
//! `McpConnectionInfo::on_connected` receives the live `HttpContext` of
//! the request that created the session, and an `HttpContext` can only
//! be produced by hyper — so unlike the rest of the middleware tests
//! this one drives an actual `MyHttpServer` over a loopback socket.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mcp_server_middleware::my_http_server::{
    HttpContext, HttpRequestHeaders, MyHttpServer, async_trait,
};
use mcp_server_middleware::{
    McpConnectionInfo, McpInputData, McpInputPayload, McpMiddleware, McpSession,
};
use parking_lot::Mutex;
use rust_extensions::{ApplicationStates, Logger};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INITIALIZE_BODY: &str = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"0.5.0"}}}"#;

#[derive(Debug, Clone, PartialEq)]
struct Connected {
    session_id: String,
    protocol_version: String,
    ip: String,
    client_header: Option<String>,
    client_name: Option<String>,
}

#[derive(Default)]
struct Recorder {
    connected: Mutex<Vec<Connected>>,
    disconnected: Mutex<Vec<String>>,
}

impl Recorder {
    fn connected(&self) -> Vec<Connected> {
        self.connected.lock().clone()
    }

    fn disconnected(&self) -> Vec<String> {
        self.disconnected.lock().clone()
    }
}

#[async_trait::async_trait]
impl McpConnectionInfo for Recorder {
    async fn on_connected(&self, session: &McpSession, ctx: &mut HttpContext) {
        let ip = ctx.request.get_ip().get_real_ip_as_string();

        let client_header = ctx
            .request
            .get_headers()
            .try_get_case_insensitive("x-test-client")
            .and_then(|value| value.as_str().ok().map(|value| value.to_string()));

        // The body was already buffered by the middleware, so re-reading
        // it here to name the client costs nothing.
        let client_name = match ctx.request.get_body().await {
            Ok(body) => McpInputPayload::try_parse(body.as_slice())
                .ok()
                .and_then(|payload| match payload.data {
                    McpInputData::Initialize(contract) => {
                        contract.client_info.and_then(|info| info.name)
                    }
                    _ => None,
                }),
            Err(_) => None,
        };

        self.connected.lock().push(Connected {
            session_id: session.id.clone(),
            protocol_version: session.version.clone(),
            ip,
            client_header,
            client_name,
        });
    }

    async fn on_disconnected(&self, session: &McpSession) {
        self.disconnected.lock().push(session.id.clone());
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

/// Starts the middleware behind a real HTTP server on a free loopback
/// port and returns the address to talk to.
async fn start_server(recorder: Arc<Recorder>) -> SocketAddr {
    let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
    mcp.register_connection_info(recorder);

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

/// Writes a raw request and returns the response head (everything up to
/// the empty line). Bodies are streamed and the connection is kept
/// alive, so reading further would just block.
async fn send_raw(addr: SocketAddr, request: String) -> String {
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

        if let Some(pos) = find_head_end(&response) {
            response.truncate(pos);
            break;
        }
    }

    String::from_utf8_lossy(&response).to_string()
}

fn find_head_end(src: &[u8]) -> Option<usize> {
    src.windows(4).position(|w| w == b"\r\n\r\n")
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

fn post(path: &str, body: &str, session_id: Option<&str>) -> String {
    let session_header = match session_id {
        Some(session_id) => format!("mcp-session-id: {}\r\n", session_id),
        None => String::new(),
    };

    format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nx-test-client: web-console\r\n{}Content-Length: {}\r\n\r\n{}",
        path,
        session_header,
        body.len(),
        body
    )
}

#[tokio::test]
async fn initialize_reports_a_connected_session_with_the_request_context() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;

    let head = send_raw(addr, post("/mcp", INITIALIZE_BODY, None)).await;
    assert_eq!(status_code(&head), 200);

    let session_id = session_header(&head).expect("initialize must return mcp-session-id");

    assert_eq!(
        recorder.connected(),
        vec![Connected {
            session_id,
            protocol_version: "2025-06-18".to_string(),
            ip: "127.0.0.1".to_string(),
            client_header: Some("web-console".to_string()),
            client_name: Some("claude-code".to_string()),
        }]
    );
    assert!(recorder.disconnected().is_empty());
}

#[tokio::test]
async fn delete_reports_the_session_as_disconnected() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;

    let head = send_raw(addr, post("/mcp", INITIALIZE_BODY, None)).await;
    let session_id = session_header(&head).expect("initialize must return mcp-session-id");

    let head = send_raw(
        addr,
        format!(
            "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\nmcp-session-id: {}\r\nContent-Length: 0\r\n\r\n",
            session_id
        ),
    )
    .await;
    assert_eq!(status_code(&head), 204);

    assert_eq!(recorder.disconnected(), vec![session_id.clone()]);
    assert_eq!(recorder.connected().len(), 1);

    // A second DELETE finds nothing to remove and stays silent.
    let head = send_raw(
        addr,
        format!(
            "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\nmcp-session-id: {}\r\nContent-Length: 0\r\n\r\n",
            session_id
        ),
    )
    .await;
    assert_eq!(status_code(&head), 404);
    assert_eq!(recorder.disconnected(), vec![session_id]);
}

#[tokio::test]
async fn lazily_created_session_is_reported_as_connected() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;

    let body = r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
    let head = send_raw(addr, post("/mcp", body, Some("client-owned-id"))).await;
    assert_eq!(status_code(&head), 200);

    let connected = recorder.connected();
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].session_id, "client-owned-id");
    assert_eq!(connected[0].ip, "127.0.0.1");
    assert_eq!(connected[0].client_header.as_deref(), Some("web-console"));
    // No `initialize` ran, so there is no clientInfo to report.
    assert!(connected[0].client_name.is_none());

    // Further requests reuse the very same session — no second event.
    let head = send_raw(addr, post("/mcp", body, Some("client-owned-id"))).await;
    assert_eq!(status_code(&head), 200);
    assert_eq!(recorder.connected().len(), 1);
}

#[tokio::test]
async fn forwarded_for_wins_over_the_socket_address() {
    let recorder = Arc::new(Recorder::default());
    let addr = start_server(recorder.clone()).await;

    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nX-Forwarded-For: 203.0.113.7, 10.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        INITIALIZE_BODY.len(),
        INITIALIZE_BODY
    );

    let head = send_raw(addr, request).await;
    assert_eq!(status_code(&head), 200);

    let connected = recorder.connected();
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].ip, "203.0.113.7");
}
