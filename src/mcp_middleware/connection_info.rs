use my_http_server::{HttpContext, async_trait};

use crate::mcp_middleware::McpSession;

/// Host hook for the MCP session lifecycle. Register it with
/// [`crate::McpMiddleware::register_connection_info`] when the host keeps
/// its own list of live MCP sessions (a "who is connected" panel, for
/// instance). Registration is optional: with no hook the middleware
/// fires nothing and does no extra work on the request path.
///
/// There are exactly two events, and *why* a session disappeared —
/// client `DELETE`, idle timeout, server restart — stays the
/// middleware's own business:
///
/// * [`Self::on_connected`] — fires exactly once per session, right
///   after it was created. Both an `initialize` and a lazily adopted
///   session id (see
///   [`crate::McpMiddleware::disabled_lazy_session_creation`]) are a
///   session appearing.
/// * [`Self::on_disconnected`] — fires at most once per session, and
///   never for a session that was not announced by `on_connected`.
///
/// Both are called with no internal lock held, so an implementation is
/// free to await, take its own locks, or call back into the middleware.
#[async_trait::async_trait]
pub trait McpConnectionInfo {
    /// A new session appeared. `ctx` is the POST request that created
    /// it, so the host can pull whatever it needs: the real client IP
    /// through `ctx.request.get_ip().get_real_ip_as_string()` (already
    /// `X-Forwarded-For`-aware), raw headers through
    /// `ctx.request.get_headers()`, and — for a session born from
    /// `initialize` — the `clientInfo` of the request body through
    /// `ctx.request.get_body()`, which is already buffered by the time
    /// this is called.
    async fn on_connected(&self, session: &McpSession, ctx: &mut HttpContext);

    /// The session is gone and will not come back under this id.
    async fn on_disconnected(&self, session: &McpSession);
}
