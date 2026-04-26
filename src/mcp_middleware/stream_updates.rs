use my_ai_agent::my_json::json_writer::JsonObjectWriter;
use my_http_server::HttpOutputProducer;

#[derive(Debug, Clone)]
pub enum McpSocketUpdateEvent {
    Shutdown,
    ToolsListChanged,
    ResourcesListChanged,
    PromptsListChanged,
}

impl McpSocketUpdateEvent {
    fn into_sse_frame(self) -> Option<Vec<u8>> {
        let method = match self {
            Self::Shutdown => return None,
            Self::ToolsListChanged => "notifications/tools/list_changed",
            Self::ResourcesListChanged => "notifications/resources/list_changed",
            Self::PromptsListChanged => "notifications/prompts/list_changed",
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
            sessions.clear_sender(session_id.as_str()).await;
            return;
        }
    }
}
