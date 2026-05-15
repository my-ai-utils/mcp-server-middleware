use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Per the MCP spec the client returns one of these three actions
/// in the `elicitation/create` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    /// Present when `action == Accept`. Shape matches the
    /// `requestedSchema` the server sent.
    pub content: Option<serde_json::Value>,
}

/// Registry of in-flight server→client `elicitation/create` requests.
///
/// When a tool wants to elicit user input it calls
/// [`Self::allocate`] which returns a fresh request id (always
/// negative, so it never collides with client-allocated ids) plus a
/// oneshot receiver. The middleware sends the JSON-RPC request to the
/// client over the SSE stream; whenever the client posts the matching
/// response, the middleware calls [`Self::resolve`], which wakes the
/// receiver.
pub struct McpElicitations {
    pending: Mutex<HashMap<i64, oneshot::Sender<ElicitationResponse>>>,
    next_id: AtomicI64,
}

impl Default for McpElicitations {
    fn default() -> Self {
        Self::new()
    }
}

impl McpElicitations {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(-1),
        }
    }

    pub fn allocate(&self) -> (i64, oneshot::Receiver<ElicitationResponse>) {
        let id = self.next_id.fetch_sub(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        (id, rx)
    }

    /// Returns `true` if there was a pending waiter for `id`. The
    /// response is dropped silently if no waiter is registered (e.g.
    /// the elicitation already timed out and was cancelled).
    pub fn resolve(&self, id: i64, response: ElicitationResponse) -> bool {
        if let Some(tx) = self.pending.lock().remove(&id) {
            let _ = tx.send(response);
            true
        } else {
            false
        }
    }

    pub fn cancel(&self, id: i64) {
        self.pending.lock().remove(&id);
    }
}

/// Parses a JSON-RPC response body (the `result` payload from the
/// client) into an [`ElicitationResponse`]. Returns `Cancel` with no
/// content if the payload is missing or malformed — the caller can
/// surface that as a tool error.
pub fn parse_elicitation_response(
    result_json: Option<&str>,
    error_json: Option<&str>,
) -> ElicitationResponse {
    if error_json.is_some() {
        return ElicitationResponse {
            action: ElicitationAction::Cancel,
            content: None,
        };
    }

    let Some(raw) = result_json else {
        return ElicitationResponse {
            action: ElicitationAction::Cancel,
            content: None,
        };
    };

    let parsed: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            return ElicitationResponse {
                action: ElicitationAction::Cancel,
                content: None,
            };
        }
    };

    let action = match parsed.get("action").and_then(|v| v.as_str()) {
        Some("accept") => ElicitationAction::Accept,
        Some("decline") => ElicitationAction::Decline,
        _ => ElicitationAction::Cancel,
    };

    let content = parsed.get("content").cloned();

    ElicitationResponse { action, content }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allocate_then_resolve_wakes_receiver() {
        let reg = McpElicitations::new();
        let (id, rx) = reg.allocate();
        assert!(id < 0, "ids must be negative to avoid client collisions");

        let resolved = reg.resolve(
            id,
            ElicitationResponse {
                action: ElicitationAction::Accept,
                content: Some(serde_json::json!({"password": "x"})),
            },
        );
        assert!(resolved);

        let got = rx.await.expect("oneshot delivered");
        assert_eq!(got.action, ElicitationAction::Accept);
    }

    #[tokio::test]
    async fn resolve_with_unknown_id_returns_false() {
        let reg = McpElicitations::new();
        let resolved = reg.resolve(
            -999,
            ElicitationResponse {
                action: ElicitationAction::Cancel,
                content: None,
            },
        );
        assert!(!resolved);
    }

    #[test]
    fn parse_accept_with_content() {
        let resp = parse_elicitation_response(
            Some(r#"{"action":"accept","content":{"password":"sekret"}}"#),
            None,
        );
        assert_eq!(resp.action, ElicitationAction::Accept);
        assert_eq!(
            resp.content.unwrap()["password"].as_str().unwrap(),
            "sekret"
        );
    }

    #[test]
    fn parse_decline_no_content() {
        let resp = parse_elicitation_response(Some(r#"{"action":"decline"}"#), None);
        assert_eq!(resp.action, ElicitationAction::Decline);
        assert!(resp.content.is_none());
    }

    #[test]
    fn parse_error_payload_treated_as_cancel() {
        let resp =
            parse_elicitation_response(None, Some(r#"{"code":-32000,"message":"oops"}"#));
        assert_eq!(resp.action, ElicitationAction::Cancel);
    }
}
