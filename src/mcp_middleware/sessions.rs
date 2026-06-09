use std::collections::{HashMap, HashSet};
use std::sync::Weak;
use std::time::Duration;

use parking_lot::Mutex;
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::mcp_middleware::McpSocketUpdateEvent;

/// How often the background GC sweeps idle sessions.
pub(crate) const GC_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

pub struct McpSession {
    pub version: String,
    pub create: DateTimeAsMicroseconds,
    pub last_access: DateTimeAsMicroseconds,
    pub sender: Option<tokio::sync::mpsc::Sender<McpSocketUpdateEvent>>,
    /// Set from the client's `capabilities.elicitation` at initialize
    /// time. Tools query this through `ToolCallContext` to decide
    /// whether to attempt `elicitation/create`.
    pub supports_elicitation: bool,
    /// Resource URIs this session subscribed to via
    /// `resources/subscribe`. `notify_resource_updated` fans
    /// `notifications/resources/updated` out to exactly these sessions.
    pub subscriptions: HashSet<String>,
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

    pub fn generate_session(
        &self,
        version: String,
        now: DateTimeAsMicroseconds,
        supports_elicitation: bool,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();

        let mut write_access = self.data.lock();

        write_access.insert(
            id.to_string(),
            McpSession {
                version,
                create: now,
                last_access: now,
                sender: None,
                supports_elicitation,
                subscriptions: HashSet::new(),
            },
        );

        id
    }

    pub fn session_supports_elicitation(&self, session_id: &str) -> bool {
        let access = self.data.lock();
        access
            .get(session_id)
            .map(|s| s.supports_elicitation)
            .unwrap_or(false)
    }

    /// Returns the SSE sender for the session, if any. Caller uses
    /// this to push targeted events (e.g. `elicitation/create`) to a
    /// single client rather than broadcasting.
    pub fn get_sender(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::mpsc::Sender<McpSocketUpdateEvent>> {
        let access = self.data.lock();
        access.get(session_id).and_then(|s| s.sender.clone())
    }

    pub fn subscribe_to_notifications(
        &self,
        session_id: &str,
        now: DateTimeAsMicroseconds,
    ) -> Option<tokio::sync::mpsc::Receiver<McpSocketUpdateEvent>> {
        let mut write_access = self.data.lock();
        let session = write_access.get_mut(session_id)?;
        session.last_access = now;
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

    /// Records a `resources/subscribe`. Returns false when the session
    /// is unknown.
    pub fn subscribe(&self, session_id: &str, uri: String) -> bool {
        let mut write_access = self.data.lock();
        if let Some(session) = write_access.get_mut(session_id) {
            session.subscriptions.insert(uri);
            return true;
        }
        false
    }

    /// Idempotent — removing an unknown subscription is a no-op.
    pub fn unsubscribe(&self, session_id: &str, uri: &str) {
        let mut write_access = self.data.lock();
        if let Some(session) = write_access.get_mut(session_id) {
            session.subscriptions.remove(uri);
        }
    }

    /// Sends `notifications/resources/updated` to every session that
    /// subscribed to `uri` and has a live SSE channel.
    pub async fn notify_resource_updated(&self, uri: &str) {
        let senders: Vec<_> = {
            let read_access = self.data.lock();
            read_access
                .values()
                .filter(|s| s.subscriptions.contains(uri))
                .filter_map(|s| s.sender.clone())
                .collect()
        };

        for sender in senders {
            let _ = sender
                .send(McpSocketUpdateEvent::ResourceUpdated {
                    uri: uri.to_string(),
                })
                .await;
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

    /// Drops sessions that have no live SSE channel and have not been
    /// touched for `idle_timeout`. Sessions with an open GET stream are
    /// never collected: a dead stream clears its own sender within a
    /// couple of keepalive intervals (send failure → `clear_sender`),
    /// after which the idle clock applies. Returns how many sessions
    /// were removed.
    pub fn remove_idle_sessions(
        &self,
        now: DateTimeAsMicroseconds,
        idle_timeout: Duration,
    ) -> usize {
        let removed: Vec<McpSession> = {
            let mut write_access = self.data.lock();

            let expired: Vec<String> = write_access
                .iter()
                .filter(|(_, session)| {
                    session.sender.is_none()
                        && now
                            .duration_since(session.last_access)
                            .as_positive_or_zero()
                            >= idle_timeout
                })
                .map(|(id, _)| id.clone())
                .collect();

            expired
                .iter()
                .filter_map(|id| write_access.remove(id))
                .collect()
        };

        // Sessions drop here, outside the lock. Their senders are None,
        // so McpSession::Drop spawns nothing.
        removed.len()
    }
}

/// Background sweeper for idle sessions. Holds a `Weak` so the task
/// dies together with the middleware instead of keeping it alive.
pub(crate) fn spawn_session_gc(sessions: Weak<McpSessions>, idle_timeout: Duration) {
    tokio::spawn(async move {
        let mut sweep = tokio::time::interval(GC_SWEEP_INTERVAL);
        // interval()'s first tick fires immediately — skip it.
        sweep.tick().await;

        loop {
            sweep.tick().await;

            let Some(sessions) = sessions.upgrade() else {
                return;
            };

            sessions.remove_idle_sessions(DateTimeAsMicroseconds::now(), idle_timeout);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_minus(seconds: i64) -> DateTimeAsMicroseconds {
        let mut result = DateTimeAsMicroseconds::now();
        result.add_seconds(-seconds);
        result
    }

    #[tokio::test]
    async fn gc_removes_only_idle_sessions_without_live_sender() {
        let sessions = McpSessions::new();
        let now = DateTimeAsMicroseconds::now();

        let idle = sessions.generate_session("2025-06-18".to_string(), now_minus(3600), false);
        let fresh = sessions.generate_session("2025-06-18".to_string(), now, false);
        let idle_with_stream =
            sessions.generate_session("2025-06-18".to_string(), now_minus(3600), false);
        let _receiver = sessions
            .subscribe_to_notifications(idle_with_stream.as_str(), now_minus(3600))
            .unwrap();

        let removed = sessions.remove_idle_sessions(now, Duration::from_secs(1800));

        assert_eq!(removed, 1);
        assert!(!sessions.check_session_and_update_last_used(idle.as_str(), now));
        assert!(sessions.check_session_and_update_last_used(fresh.as_str(), now));
        assert!(sessions.check_session_and_update_last_used(idle_with_stream.as_str(), now));
    }

    #[tokio::test]
    async fn resource_updated_reaches_only_subscribed_sessions() {
        let sessions = McpSessions::new();
        let now = DateTimeAsMicroseconds::now();

        let subscribed = sessions.generate_session("2025-06-18".to_string(), now, false);
        let other = sessions.generate_session("2025-06-18".to_string(), now, false);

        let mut subscribed_rx = sessions
            .subscribe_to_notifications(subscribed.as_str(), now)
            .unwrap();
        let mut other_rx = sessions
            .subscribe_to_notifications(other.as_str(), now)
            .unwrap();

        assert!(sessions.subscribe(subscribed.as_str(), "res://a".to_string()));
        assert!(!sessions.subscribe("unknown-session", "res://a".to_string()));

        sessions.notify_resource_updated("res://a").await;

        match subscribed_rx.try_recv() {
            Ok(McpSocketUpdateEvent::ResourceUpdated { uri }) => assert_eq!(uri, "res://a"),
            other => panic!("expected ResourceUpdated, got {:?}", other),
        }
        assert!(other_rx.try_recv().is_err());

        // After unsubscribe no more events arrive.
        sessions.unsubscribe(subscribed.as_str(), "res://a");
        sessions.notify_resource_updated("res://a").await;
        assert!(subscribed_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn get_stream_refreshes_last_access() {
        let sessions = McpSessions::new();

        let session_id =
            sessions.generate_session("2025-06-18".to_string(), now_minus(3600), false);

        let now = DateTimeAsMicroseconds::now();
        let receiver = sessions.subscribe_to_notifications(session_id.as_str(), now);
        assert!(receiver.is_some());
        drop(receiver);

        // Sender is live, so even an aggressive sweep must keep it; and
        // last_access was just refreshed.
        sessions.clear_sender(session_id.as_str());
        let removed = sessions.remove_idle_sessions(now, Duration::from_secs(1800));
        assert_eq!(removed, 0);
    }
}
