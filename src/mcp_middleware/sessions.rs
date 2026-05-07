use std::collections::HashMap;

use parking_lot::Mutex;
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::mcp_middleware::McpSocketUpdateEvent;

pub struct McpSession {
    pub version: String,
    pub create: DateTimeAsMicroseconds,
    pub last_access: DateTimeAsMicroseconds,
    pub sender: Option<tokio::sync::mpsc::Sender<McpSocketUpdateEvent>>,
}

impl Drop for McpSession {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            tokio::spawn(async move {
                let _ = sender.send(McpSocketUpdateEvent::Shutdown).await;
            });
        }
    }
}

pub struct McpSessions {
    data: Mutex<HashMap<String, McpSession>>,
}

impl McpSessions {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }

    pub fn generate_session(&self, version: String, now: DateTimeAsMicroseconds) -> String {
        let id = uuid::Uuid::new_v4().to_string();

        let mut write_access = self.data.lock();

        write_access.insert(
            id.to_string(),
            McpSession {
                version,
                create: now,
                last_access: now,
                sender: None,
            },
        );

        id
    }

    pub fn subscribe_to_notifications(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::mpsc::Receiver<McpSocketUpdateEvent>> {
        let mut write_access = self.data.lock();
        let session = write_access.get_mut(session_id)?;
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        session.sender = Some(sender);
        Some(receiver)
    }

    pub fn check_session_and_update_last_used(
        &self,
        session_id: &str,
        now: DateTimeAsMicroseconds,
    ) -> bool {
        let mut write_access = self.data.lock();

        if let Some(session) = write_access.get_mut(session_id) {
            session.last_access = now;
            return true;
        }

        false
    }

    pub fn delete_session(&self, session_id: &str) -> bool {
        let mut write_access = self.data.lock();
        write_access.remove(session_id).is_some()
    }

    pub fn clear_sender(&self, session_id: &str) {
        let mut write_access = self.data.lock();
        if let Some(session) = write_access.get_mut(session_id) {
            session.sender = None;
        }
    }

    pub async fn broadcast(&self, event: McpSocketUpdateEvent) {
        let senders: Vec<_> = {
            let read_access = self.data.lock();
            read_access
                .values()
                .filter_map(|s| s.sender.clone())
                .collect()
        };

        for sender in senders {
            let _ = sender.send(event.clone()).await;
        }
    }
}
