use std::time::Duration;

use my_ai_agent::my_json::json_writer::{JsonObjectWriter, RawJsonObject};
use my_http_server::HttpOutputProducer;

/// Interval between SSE comment frames sent on an otherwise idle stream.
/// Without them an idle SSE connection produces no bytes for minutes, so
/// neither the client nor any reverse proxy in between can tell when the
/// socket has actually died (TCP keepalive is OS-level and slow). 15s is
/// short enough to survive aggressive proxy idle timeouts and long enough
/// not to add meaningful traffic.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Initial `retry:` hint sent to EventSource-style clients so reconnect
/// backoff has a sane default even if the client doesn't pick one.
const SSE_RETRY_MS: u64 = 3000;

#[derive(Debug, Clone)]
pub enum McpSocketUpdateEvent {
    Shutdown,
    ToolsListChanged,
    ResourcesListChanged,
    PromptsListChanged,
    /// `notifications/resources/updated` — sent to sessions that
    /// subscribed to the URI via `resources/subscribe`.
    ResourceUpdated { uri: String },
    /// Server→client request asking the client to elicit user input.
    /// Carries a server-allocated request id (negative, distinct from
    /// client-allocated ids), a prompt message and a pre-serialized
    /// JSON schema describing the expected shape of the user's reply.
    ElicitationRequest {
        id: i64,
        message: String,
        requested_schema: String,
    },
}

impl McpSocketUpdateEvent {
    fn into_sse_frame(self) -> Option<Vec<u8>> {
        match self {
            Self::Shutdown => return None,
            Self::ElicitationRequest {
                id,
                message,
                requested_schema,
            } => {
                let mut frame = "data: ".to_string();
                JsonObjectWriter::new()
                    .write("jsonrpc", "2.0")
                    .write("id", id)
                    .write("method", "elicitation/create")
                    .write_json_object("params", |p| {
                        p.write("message", message.as_str())
                            .write("requestedSchema", RawJsonObject::AsStr(&requested_schema))
                    })
                    .build_into(&mut frame);
                frame.push('\n');
                frame.push('\n');
                return Some(frame.into_bytes());
            }
            Self::ResourceUpdated { uri } => {
                let mut frame = "data: ".to_string();
                JsonObjectWriter::new()
                    .write("jsonrpc", "2.0")
                    .write("method", "notifications/resources/updated")
                    .write_json_object("params", |p| p.write("uri", uri.as_str()))
                    .build_into(&mut frame);
                frame.push('\n');
                frame.push('\n');
                return Some(frame.into_bytes());
            }
            _ => {}
        }

        let method = match self {
            Self::ToolsListChanged => "notifications/tools/list_changed",
            Self::ResourcesListChanged => "notifications/resources/list_changed",
            Self::PromptsListChanged => "notifications/prompts/list_changed",
            Self::Shutdown | Self::ElicitationRequest { .. } | Self::ResourceUpdated { .. } => {
                unreachable!()
            }
        };

        let mut frame = "data: ".to_string();
        JsonObjectWriter::new()
            .write("jsonrpc", "2.0")
            .write("method", method)
            .write_json_object("params", |p| p)
            .build_into(&mut frame);
        frame.push('\n');
        frame.push('\n');

        Some(frame.into_bytes())
    }
}

pub async fn stream_updates(
    mut producer: HttpOutputProducer,
    mut receiver: tokio::sync::mpsc::Receiver<McpSocketUpdateEvent>,
    sessions: std::sync::Arc<super::McpSessions>,
    session_id: String,
) {
    // Kick the stream immediately so reverse proxies that buffer until
    // the first byte flush response headers downstream, and so EventSource
    // clients get a reconnect-backoff hint.
    let preamble = format!("retry: {}\n\n", SSE_RETRY_MS).into_bytes();
    if producer.send(preamble).await.is_err() {
        sessions.clear_sender(session_id.as_str());
        return;
    }

    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    // interval()'s first tick fires immediately; skip it so we don't emit
    // a comment right after the preamble.
    keepalive.tick().await;

    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    return;
                };
                let Some(frame) = event.into_sse_frame() else {
                    return;
                };
                if producer.send(frame).await.is_err() {
                    sessions.clear_sender(session_id.as_str());
                    return;
                }
            }
            _ = keepalive.tick() => {
                // SSE comment line — ignored by the client per spec, but
                // forces a write so a broken socket is detected promptly
                // and intermediaries keep the connection alive.
                if producer.send(b": keepalive\n\n".to_vec()).await.is_err() {
                    sessions.clear_sender(session_id.as_str());
                    return;
                }
            }
        }
    }
}
