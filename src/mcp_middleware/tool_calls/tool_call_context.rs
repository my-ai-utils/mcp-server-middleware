use std::sync::Arc;
use std::time::Duration;

use crate::mcp_middleware::{
    ElicitationResponse, McpElicitations, McpSessions, McpSocketUpdateEvent,
};

/// Per-call context handed to tools that opt in to context-aware
/// execution via [`super::McpToolCallEx`]. Lets the tool reach back
/// to the client for things the model can't or shouldn't see — most
/// notably MCP elicitation (`elicitation/create`).
pub struct ToolCallContext {
    pub session_id: String,
    pub supports_elicitation: bool,
    pub(crate) elicitations: Arc<McpElicitations>,
    pub(crate) sessions: Arc<McpSessions>,
}

impl ToolCallContext {
    /// Server→client `elicitation/create` request. Asks the connected
    /// client to prompt the user for input matching `requested_schema`.
    ///
    /// Errors:
    /// - client did not advertise `capabilities.elicitation` at init
    /// - no live SSE channel for this session
    /// - user did not reply within `timeout`
    pub async fn elicit(
        &self,
        message: &str,
        requested_schema: serde_json::Value,
        timeout: Duration,
    ) -> Result<ElicitationResponse, String> {
        if !self.supports_elicitation {
            return Err("MCP client does not support elicitation".to_string());
        }

        let sender = self
            .sessions
            .get_sender(&self.session_id)
            .ok_or_else(|| "No active SSE channel for this MCP session".to_string())?;

        let (id, rx) = self.elicitations.allocate();

        let requested_schema_str = serde_json::to_string(&requested_schema)
            .map_err(|e| format!("Can not serialize elicitation schema: {}", e))?;

        let event = McpSocketUpdateEvent::ElicitationRequest {
            id,
            message: message.to_string(),
            requested_schema: requested_schema_str,
        };

        if sender.send(event).await.is_err() {
            self.elicitations.cancel(id);
            return Err("Failed to deliver elicitation/create — SSE channel closed".to_string());
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err("Elicitation channel dropped before client replied".to_string()),
            Err(_) => {
                self.elicitations.cancel(id);
                Err("Elicitation timed out — client did not reply in time".to_string())
            }
        }
    }
}
