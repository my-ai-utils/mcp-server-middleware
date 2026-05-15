use my_ai_agent::my_json::json_writer::{JsonObjectWriter, RawJsonObject};
use my_http_server::HttpOutputProducer;

#[derive(Debug, Clone)]
pub enum McpSocketUpdateEvent {
    Shutdown,
    ToolsListChanged,
    ResourcesListChanged,
    PromptsListChanged,
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
            _ => {}
        }

        let method = match self {
            Self::ToolsListChanged => "notifications/tools/list_changed",
            Self::ResourcesListChanged => "notifications/resources/list_changed",
            Self::PromptsListChanged => "notifications/prompts/list_changed",
            Self::Shutdown | Self::ElicitationRequest { .. } => unreachable!(),
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
    while let Some(event) = receiver.recv().await {
        let Some(frame) = event.into_sse_frame() else {
            return;
        };

        if producer.send(frame).await.is_err() {
            sessions.clear_sender(session_id.as_str());
            return;
        }
    }
}
