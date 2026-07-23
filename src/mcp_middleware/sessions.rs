use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use my_http_server::HttpContext;
use parking_lot::Mutex;
use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::mcp_middleware::{McpConnectionInfo, McpSocketUpdateEvent};

/// How often the background GC sweeps idle sessions.
pub(crate) const GC_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// An MCP session as the outside world sees it: plain, cheap-to-clone
/// data. The mutable runtime state — SSE channel, idle clock,
/// subscriptions — stays in the private [`SessionEntry`], so handing an
/// `McpSession` to a host can neither keep a stream alive nor run
/// `Drop` in the host's hands.
#[derive(Debug, Clone)]
pub struct McpSession {
    pub id: String,
    pub version: String,
    pub create: DateTimeAsMicroseconds,
    /// Set from the client's `capabilities.elicitation` at initialize
    /// time. Tools query this through `ToolCallContext` to decide
    /// whether to attempt `elicitation/create`.
    pub supports_elicitation: bool,
}

/// What the sessions map actually stores.
struct SessionEntry {
    session: McpSession,
    last_access: DateTimeAsMicroseconds,
    sender: Option<tokio::sync::mpsc::Sender<McpSocketUpdateEvent>>,
    /// Resource URIs this session subscribed to via
    /// `resources/subscribe`. `notify_resource_updated` fans
    /// `notifications/resources/updated` out to exactly these sessions.
    subscriptions: HashSet<String>,
}

impl SessionEntry {
    fn new(session: McpSession, now: DateTimeAsMicroseconds) -> Self {
        Self {
            session,
            last_access: now,
            sender: None,
            subscriptions: HashSet::new(),
        }
    }
}

impl Drop for SessionEntry {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            tokio::spawn(async move {
                let _ = sender.send(McpSocketUpdateEvent::Shutdown).await;
            });
        }
    }
}

pub struct McpSessions {
    data: Mutex<HashMap<String, SessionEntry>>,
    /// Optional host hook for session lifecycle events. Set once at
    /// start-up, before the server serves anything, so reading it is a
    /// plain atomic load — a host that registers nothing pays nothing.
    connection_info: OnceLock<Arc<dyn McpConnectionInfo + Send + Sync + 'static>>,
}

impl McpSessions {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            connection_info: OnceLock::new(),
        }
    }

    /// Installs the host hook. Only the first call wins.
    pub(crate) fn set_connection_info(
        &self,
        connection_info: Arc<dyn McpConnectionInfo + Send + Sync + 'static>,
    ) {
        let _ = self.connection_info.set(connection_info);
    }

    pub(crate) fn has_connection_info(&self) -> bool {
        self.connection_info.get().is_some()
    }

    /// Announces a session that was just inserted into the map. The only
    /// callers are the two creation paths in the middleware, which is
    /// what keeps the event exactly-once per session.
    pub(crate) async fn notify_connected(&self, session: &McpSession, ctx: &mut HttpContext) {
        if let Some(connection_info) = self.connection_info.get() {
            connection_info.on_connected(session, ctx).await;
        }
    }

    /// Announces a session that was really removed from the map, with
    /// `data` unlocked. Hooking `Drop for SessionEntry` instead would
    /// look tempting — one place covers every removal path — but
    /// `remove_idle_sessions` drops entries while holding `data.lock()`,
    /// so host code would run under our mutex and could deadlock.
    async fn notify_disconnected(&self, session: &McpSession) {
        if let Some(connection_info) = self.connection_info.get() {
            connection_info.on_disconnected(session).await;
        }
    }

    /// Mints a session with a server-generated id and returns it, so the
    /// caller can both answer the client and announce the new session
    /// without looking it up again.
    pub fn generate_session(
        &self,
        version: String,
        now: DateTimeAsMicroseconds,
        supports_elicitation: bool,
    ) -> McpSession {
        let session = McpSession {
            id: uuid::Uuid::new_v4().to_string(),
            version,
            create: now,
            supports_elicitation,
        };

        let mut write_access = self.data.lock();

        write_access.insert(session.id.clone(), SessionEntry::new(session.clone(), now));

        session
    }

    /// Registers a session under a client-supplied id (lazy session
    /// creation). Never overwrites an existing session — a concurrent
    /// request that already created it just refreshes `last_access`.
    /// Returns the session only when a new one was actually minted.
    pub fn ensure_session_with_id(
        &self,
        session_id: &str,
        version: String,
        now: DateTimeAsMicroseconds,
        supports_elicitation: bool,
    ) -> Option<McpSession> {
        let mut write_access = self.data.lock();

        if let Some(entry) = write_access.get_mut(session_id) {
            entry.last_access = now;
            return None;
        }

        let session = McpSession {
            id: session_id.to_string(),
            version,
            create: now,
            supports_elicitation,
        };

        write_access.insert(session.id.clone(), SessionEntry::new(session.clone(), now));

        Some(session)
    }

    pub fn session_supports_elicitation(&self, session_id: &str) -> bool {
        let access = self.data.lock();
        access
            .get(session_id)
            .map(|entry| entry.session.supports_elicitation)
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

    pub async fn delete_session(&self, session_id: &str) -> bool {
        let removed = {
            let mut write_access = self.data.lock();
            write_access.remove(session_id)
        };

        let Some(entry) = removed else {
            return false;
        };

        let session = entry.session.clone();

        // Release the entry (and with it the SSE shutdown its Drop
        // sends) before handing control to the host.
        drop(entry);

        self.notify_disconnected(&session).await;

        true
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
    pub async fn remove_idle_sessions(
        &self,
        now: DateTimeAsMicroseconds,
        idle_timeout: Duration,
    ) -> usize {
        let removed: Vec<SessionEntry> = {
            let mut write_access = self.data.lock();

            let expired: Vec<String> = write_access
                .iter()
                .filter(|(_, entry)| {
                    entry.sender.is_none()
                        && now.duration_since(entry.last_access).as_positive_or_zero()
                            >= idle_timeout
                })
                .map(|(id, _)| id.clone())
                .collect();

            expired
                .iter()
                .filter_map(|id| write_access.remove(id))
                .collect()
        };

        // The sessions are copied out and the entries dropped here,
        // outside the lock. Their senders are None, so
        // SessionEntry::Drop spawns nothing.
        let removed: Vec<McpSession> = removed
            .into_iter()
            .map(|entry| entry.session.clone())
            .collect();

        let result = removed.len();

        for session in removed {
            self.notify_disconnected(&session).await;
        }

        result
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

            sessions
                .remove_idle_sessions(DateTimeAsMicroseconds::now(), idle_timeout)
                .await;
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
            .subscribe_to_notifications(idle_with_stream.id.as_str(), now_minus(3600))
            .unwrap();

        let removed = sessions
            .remove_idle_sessions(now, Duration::from_secs(1800))
            .await;

        assert_eq!(removed, 1);
        assert!(!sessions.check_session_and_update_last_used(idle.id.as_str(), now));
        assert!(sessions.check_session_and_update_last_used(fresh.id.as_str(), now));
        assert!(sessions.check_session_and_update_last_used(idle_with_stream.id.as_str(), now));
    }

    #[tokio::test]
    async fn resource_updated_reaches_only_subscribed_sessions() {
        let sessions = McpSessions::new();
        let now = DateTimeAsMicroseconds::now();

        let subscribed = sessions.generate_session("2025-06-18".to_string(), now, false);
        let other = sessions.generate_session("2025-06-18".to_string(), now, false);

        let mut subscribed_rx = sessions
            .subscribe_to_notifications(subscribed.id.as_str(), now)
            .unwrap();
        let mut other_rx = sessions
            .subscribe_to_notifications(other.id.as_str(), now)
            .unwrap();

        assert!(sessions.subscribe(subscribed.id.as_str(), "res://a".to_string()));
        assert!(!sessions.subscribe("unknown-session", "res://a".to_string()));

        sessions.notify_resource_updated("res://a").await;

        match subscribed_rx.try_recv() {
            Ok(McpSocketUpdateEvent::ResourceUpdated { uri }) => assert_eq!(uri, "res://a"),
            other => panic!("expected ResourceUpdated, got {:?}", other),
        }
        assert!(other_rx.try_recv().is_err());

        // After unsubscribe no more events arrive.
        sessions.unsubscribe(subscribed.id.as_str(), "res://a");
        sessions.notify_resource_updated("res://a").await;
        assert!(subscribed_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn get_stream_refreshes_last_access() {
        let sessions = McpSessions::new();

        let session = sessions.generate_session("2025-06-18".to_string(), now_minus(3600), false);

        let now = DateTimeAsMicroseconds::now();
        let receiver = sessions.subscribe_to_notifications(session.id.as_str(), now);
        assert!(receiver.is_some());
        drop(receiver);

        // Sender is live, so even an aggressive sweep must keep it; and
        // last_access was just refreshed.
        sessions.clear_sender(session.id.as_str());
        let removed = sessions
            .remove_idle_sessions(now, Duration::from_secs(1800))
            .await;
        assert_eq!(removed, 0);
    }
}
