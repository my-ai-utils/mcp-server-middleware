use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use my_http_server::async_trait;
use rust_extensions::date_time::DateTimeAsMicroseconds;

/// Every way a request can fail inside the middleware, as an event a host
/// can put in its own log or activity console.
///
/// This is meant to be exhaustive: if the middleware refuses a request or
/// something goes wrong while it serves one, it is one of the variants
/// below. The only deliberate omissions are a client that goes away
/// mid-stream (an SSE write failing is a disconnect, not an error — it
/// would fire on every closed tab) and errors a tool returns to itself,
/// such as a failed [`crate::ToolCallContext::elicit`], which the tool
/// decides what to do with and which show up here as
/// [`Self::ToolExecution`] only if it propagates them.
///
/// Borrowed on purpose: the hook is awaited on the request path and the
/// host copies only what it wants to keep. That also makes the whole
/// enum `Copy`, so the middleware can write it to the console and still
/// hand it to the hook.
///
/// `session_id` is carried wherever the middleware knows it — a host
/// serving several endpoints needs it to attribute the line to the
/// session that produced it. It is an `Option` only on the three variants
/// that can fire before a session is established.
#[derive(Debug, Clone, Copy)]
pub enum McpMiddlewareError<'s> {
    /// `tools/call` arguments did not match the tool's `inputSchema`, so
    /// the tool never ran. `arguments` is the raw JSON the client sent
    /// and `error` the deserializer's complaint.
    ToolInputDeserialization {
        session_id: &'s str,
        tool_name: &'s str,
        arguments: &'s str,
        error: &'s str,
    },
    /// The tool ran and returned `Err`. Note that a call whose arguments
    /// fail to deserialize also lands here, right after
    /// [`Self::ToolInputDeserialization`] — a bad-arguments call fails
    /// twice on its way out and is reported twice.
    ToolExecution {
        session_id: &'s str,
        tool_name: &'s str,
        arguments: &'s str,
        error: &'s str,
    },
    /// `tools/call` named a tool that is not registered.
    ToolNotFound {
        session_id: &'s str,
        tool_name: &'s str,
        arguments: &'s str,
    },
    /// The tool succeeded but its `OutputData` could not be serialized,
    /// so there is nothing to answer with. A bug in the tool's own types
    /// (a map with non-string keys, a `NaN`), not in the client's request.
    ToolOutputSerialization {
        session_id: &'s str,
        tool_name: &'s str,
        arguments: &'s str,
        error: &'s str,
    },
    /// A prompt returned `Err`.
    PromptExecution {
        session_id: &'s str,
        prompt_name: &'s str,
        arguments: &'s HashMap<String, String>,
        error: &'s str,
    },
    /// `prompts/get` named a prompt that is not registered.
    PromptNotFound {
        session_id: &'s str,
        prompt_name: &'s str,
        arguments: &'s HashMap<String, String>,
    },
    /// `resources/read` or `resources/subscribe` named a URI that is
    /// neither a static nor a dynamic resource. `method` says which of
    /// the two asked.
    ResourceNotFound {
        session_id: &'s str,
        method: &'s str,
        uri: &'s str,
    },
    /// The resource exists and its `read_resource` returned `Err`.
    ResourceRead {
        session_id: &'s str,
        uri: &'s str,
        error: &'s str,
    },
    /// The JSON-RPC envelope or the `params` of a known method could not
    /// be parsed at all.
    ///
    /// `session_id` is `None` when the request carried no
    /// `mcp-session-id` header. `method` is the method the payload named
    /// and is empty when the failure happened before the middleware got
    /// that far (unreadable JSON, missing `jsonrpc`, missing `method`).
    /// `payload` is the fragment that failed: the `params` object for a
    /// per-method failure, the whole request body for an envelope one.
    PayloadDeserialization {
        session_id: Option<&'s str>,
        method: &'s str,
        payload: &'s str,
        error: &'s str,
    },
    /// A well-formed request naming a method this middleware does not
    /// implement. A request (`id` present) is answered with
    /// `-32601 Method not found`; a notification is silently accepted with
    /// `202`, and is reported here all the same — an ignored notification
    /// is exactly the kind of thing a host wants to find out about.
    MethodNotFound {
        session_id: &'s str,
        method: &'s str,
        payload: &'s str,
    },
    /// The request was refused before any MCP method ran, because of the
    /// `mcp-session-id` header: it was missing (`400`), or it named a
    /// session the server does not have and lazy session creation is off
    /// (`404`). `http_method` is `POST`, `GET` or `DELETE` — a `GET` here
    /// is a client failing to open its SSE stream, a `DELETE` a client
    /// closing a session twice.
    ///
    /// This one is debug info rather than an error, see
    /// [`Self::is_debug_info`]: it is written to the console by a debug
    /// build only.
    SessionRejected {
        session_id: Option<&'s str>,
        http_method: &'s str,
        error: &'s str,
    },
    /// The request body could not be read off the socket, so the
    /// middleware never saw a payload to parse.
    RequestBodyRead {
        session_id: Option<&'s str>,
        error: &'s str,
    },
}

impl<'s> McpMiddlewareError<'s> {
    /// The session that produced the event, when the middleware knows it.
    /// Only the three variants that can fire before a session exists —
    /// [`Self::PayloadDeserialization`], [`Self::SessionRejected`] and
    /// [`Self::RequestBodyRead`] — can answer `None`.
    pub fn session_id(&self) -> Option<&'s str> {
        match self {
            Self::ToolInputDeserialization { session_id, .. }
            | Self::ToolExecution { session_id, .. }
            | Self::ToolNotFound { session_id, .. }
            | Self::ToolOutputSerialization { session_id, .. }
            | Self::PromptExecution { session_id, .. }
            | Self::PromptNotFound { session_id, .. }
            | Self::ResourceNotFound { session_id, .. }
            | Self::ResourceRead { session_id, .. }
            | Self::MethodNotFound { session_id, .. } => Some(session_id),
            Self::PayloadDeserialization { session_id, .. }
            | Self::SessionRejected { session_id, .. }
            | Self::RequestBodyRead { session_id, .. } => *session_id,
        }
    }

    /// `true` for the events that are routine rather than errors — today
    /// that is [`Self::SessionRejected`]: a client showing up with a
    /// session the server no longer has (a restart, the idle GC), or with
    /// no session at all. In production that happens all the time and is
    /// fine: the client is answered `404`/`400` and starts over, and
    /// there is nothing to fix on this side.
    ///
    /// The middleware writes these to the console in a debug build only —
    /// a release build does not have that code at all. The host hook
    /// still gets them, and can use this to file them apart from the
    /// real errors or to drop them.
    pub fn is_debug_info(&self) -> bool {
        matches!(self, Self::SessionRejected { .. })
    }
}

/// A one-line rendering — what the middleware itself writes to stderr,
/// and what a host that just wants to log the event can reuse. It is
/// deliberately *not* the text the client receives — that stays exactly
/// what it was, see [`McpMiddlewareErrorHook`].
impl Display for McpMiddlewareError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolInputDeserialization {
                tool_name,
                arguments,
                error,
                ..
            } => write!(
                f,
                "Tool `{}` got arguments that do not match its inputSchema: {}. Arguments: {}",
                tool_name, error, arguments
            ),
            Self::ToolExecution {
                tool_name,
                arguments,
                error,
                ..
            } => write!(
                f,
                "Tool `{}` failed: {}. Arguments: {}",
                tool_name, error, arguments
            ),
            Self::ToolNotFound {
                tool_name,
                arguments,
                ..
            } => write!(
                f,
                "Tool `{}` is not registered. Arguments: {}",
                tool_name, arguments
            ),
            Self::ToolOutputSerialization {
                tool_name,
                arguments,
                error,
                ..
            } => write!(
                f,
                "Tool `{}` produced a result that can not be serialized: {}. Arguments: {}",
                tool_name, error, arguments
            ),
            Self::PromptExecution {
                prompt_name,
                arguments,
                error,
                ..
            } => write!(
                f,
                "Prompt `{}` failed: {}. Arguments: {:?}",
                prompt_name, error, arguments
            ),
            Self::PromptNotFound {
                prompt_name,
                arguments,
                ..
            } => write!(
                f,
                "Prompt `{}` is not registered. Arguments: {:?}",
                prompt_name, arguments
            ),
            Self::ResourceNotFound { method, uri, .. } => write!(
                f,
                "Resource `{}` is not registered, asked for by `{}`",
                uri, method
            ),
            Self::ResourceRead { uri, error, .. } => {
                write!(f, "Reading the resource `{}` failed: {}", uri, error)
            }
            Self::PayloadDeserialization {
                method,
                payload,
                error,
                ..
            } => write!(
                f,
                "Can not deserialize the `{}` payload: {}. Payload: {}",
                method, error, payload
            ),
            Self::MethodNotFound {
                method, payload, ..
            } => write!(
                f,
                "Method `{}` is not supported. Payload: {}",
                method, payload
            ),
            Self::SessionRejected {
                http_method, error, ..
            } => write!(f, "{} request refused: {}", http_method, error),
            Self::RequestBodyRead { error, .. } => {
                write!(f, "Can not read the request body: {}", error)
            }
        }
    }
}

/// Host hook for the errors the middleware runs into. Register it with
/// [`crate::McpMiddleware::register_error_hook`] when the application
/// keeps its own activity log or console — without it these events only
/// ever reach stderr, where an embedding application can not see them.
///
/// The hook is **additional**, not a replacement: every error goes to
/// stderr as well, prefixed with the timestamp and `McpMiddleware`, so a
/// plain `docker logs` still shows it. Registering a hook changes nothing
/// about that.
///
/// The hook sees more than the console does: the events that are
/// [`McpMiddlewareError::is_debug_info`] reach stderr in a debug build
/// only, and reach the hook always.
///
/// This is a pure observation hook. It can neither alter nor suppress a
/// response: the error text sent to the client, the JSON-RPC codes and
/// the SSE framing are decided before and independently of it.
///
/// `on_error` is awaited on the request path, so it is expected to be
/// **cheap** — push into a ring buffer, send on an unbounded channel,
/// bump a counter. Anything slow (a network call, a database write)
/// belongs in a task the hook spawns, not in the hook itself.
///
/// A panicking hook can not take the request down: the panic is caught
/// and the event has already been written to stderr by then.
#[async_trait::async_trait]
pub trait McpMiddlewareErrorHook {
    async fn on_error(&self, err: McpMiddlewareError<'_>);
}

/// Writes every error the middleware runs into to stderr, and hands it to
/// the host hook when one is registered. The hook is set once at
/// start-up, before the server serves anything, so reading it on the
/// error path is a plain atomic load — a host that registers nothing
/// pays nothing.
pub(crate) struct McpErrorReporter {
    hook: OnceLock<Arc<dyn McpMiddlewareErrorHook + Send + Sync + 'static>>,
}

impl McpErrorReporter {
    pub(crate) fn new() -> Self {
        Self {
            hook: OnceLock::new(),
        }
    }

    /// Installs the host hook. Only the first call wins.
    pub(crate) fn set_hook(&self, hook: Arc<dyn McpMiddlewareErrorHook + Send + Sync + 'static>) {
        let _ = self.hook.set(hook);
    }

    /// Reports `err` to both places: stderr — always for an error, in a
    /// debug build only for debug info — and the host hook when one is
    /// registered. The console line goes out first, so a slow or broken
    /// hook can not delay or swallow it.
    pub(crate) async fn report(&self, err: McpMiddlewareError<'_>) {
        write_to_console(&err);

        if let Some(hook) = self.hook.get() {
            CatchPanic::new(hook.on_error(err)).await;
        }
    }
}

fn write_to_console(err: &McpMiddlewareError<'_>) {
    if !err.is_debug_info() {
        eprintln!(
            "{} McpMiddleware error: {}",
            DateTimeAsMicroseconds::now().to_rfc3339(),
            err
        );
        return;
    }

    // Debug info is routine in production, so a release build is not
    // even compiled with the line below.
    #[cfg(debug_assertions)]
    eprintln!(
        "{} McpMiddleware debug: {}",
        DateTimeAsMicroseconds::now().to_rfc3339(),
        err
    );
}

/// Runs a host hook so a panic inside it can not take the request down.
/// The panic still reaches the process panic hook — the usual backtrace
/// on stderr — we only need to learn that it happened, so the caller can
/// fall back to printing.
struct CatchPanic<F> {
    inner: Pin<Box<F>>,
    panicked: bool,
}

impl<F: Future<Output = ()>> CatchPanic<F> {
    fn new(inner: F) -> Self {
        Self {
            inner: Box::pin(inner),
            panicked: false,
        }
    }
}

impl<F: Future<Output = ()>> Future for CatchPanic<F> {
    /// `true` — the hook completed; `false` — it panicked.
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<bool> {
        // Every field is `Unpin` — `F` only ever appears behind a `Box`
        // — so the outer pin projects for free, without `unsafe`.
        let this = self.get_mut();

        // A future that panicked must never be polled again.
        if this.panicked {
            return Poll::Ready(false);
        }

        let inner = &mut this.inner;

        match catch_unwind(AssertUnwindSafe(|| inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(())) => Poll::Ready(true),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => {
                this.panicked = true;
                Poll::Ready(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder {
        seen: parking_lot::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl McpMiddlewareErrorHook for Recorder {
        async fn on_error(&self, err: McpMiddlewareError<'_>) {
            // An await point before the record, so a hook that yields is
            // covered too.
            tokio::task::yield_now().await;
            self.seen.lock().push(err.to_string());
        }
    }

    struct Panicking;

    #[async_trait::async_trait]
    impl McpMiddlewareErrorHook for Panicking {
        async fn on_error(&self, _err: McpMiddlewareError<'_>) {
            tokio::task::yield_now().await;
            panic!("host hook is broken");
        }
    }

    fn error() -> McpMiddlewareError<'static> {
        McpMiddlewareError::ToolNotFound {
            session_id: "s1",
            tool_name: "nope",
            arguments: "{}",
        }
    }

    fn session_rejected() -> McpMiddlewareError<'static> {
        McpMiddlewareError::SessionRejected {
            session_id: Some("s1"),
            http_method: "GET",
            error: "Unknown MCP session",
        }
    }

    /// A refused session is routine, anything else is an error.
    #[test]
    fn only_a_rejected_session_is_debug_info() {
        assert!(session_rejected().is_debug_info());
        assert!(!error().is_debug_info());
    }

    /// Keeping debug info off the console must not keep it from the host
    /// — in a release build the hook is the only place it shows up.
    #[tokio::test]
    async fn the_hook_gets_debug_info() {
        let reporter = McpErrorReporter::new();
        let recorder = Arc::new(Recorder {
            seen: parking_lot::Mutex::new(Vec::new()),
        });
        reporter.set_hook(recorder.clone());

        reporter.report(session_rejected()).await;
        assert_eq!(
            recorder.seen.lock().as_slice(),
            ["GET request refused: Unknown MCP session"]
        );
    }

    /// With nothing registered the report is still made — to stderr — and
    /// must not panic or block.
    #[tokio::test]
    async fn reporting_without_a_hook_is_a_no_op_for_the_caller() {
        let reporter = McpErrorReporter::new();
        reporter.report(error()).await;
    }

    #[tokio::test]
    async fn a_registered_hook_gets_the_event() {
        let reporter = McpErrorReporter::new();
        let recorder = Arc::new(Recorder {
            seen: parking_lot::Mutex::new(Vec::new()),
        });
        reporter.set_hook(recorder.clone());

        reporter.report(error()).await;
        assert_eq!(recorder.seen.lock().len(), 1);
        assert!(recorder.seen.lock()[0].contains("`nope` is not registered"));
    }

    #[tokio::test]
    async fn only_the_first_registration_is_kept() {
        let reporter = McpErrorReporter::new();
        let first = Arc::new(Recorder {
            seen: parking_lot::Mutex::new(Vec::new()),
        });
        let second = Arc::new(Recorder {
            seen: parking_lot::Mutex::new(Vec::new()),
        });
        reporter.set_hook(first.clone());
        reporter.set_hook(second.clone());

        reporter.report(error()).await;
        assert_eq!(first.seen.lock().len(), 1);
        assert!(second.seen.lock().is_empty());
    }

    /// A panicking host hook must not propagate — the request goes on and
    /// the console line has already been written by then.
    #[tokio::test]
    async fn a_panicking_hook_does_not_propagate() {
        let reporter = McpErrorReporter::new();
        reporter.set_hook(Arc::new(Panicking));

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        reporter.report(error()).await;
        std::panic::set_hook(previous);
    }
}
