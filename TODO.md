# TODO — the road to a complete MCP server

What is still left to do in `mcp-server-middleware` so that a downstream service which depends on this crate alone is a complete MCP server per the `2025-11-25` spec.

## Protocol

- [ ] **`resources/subscribe` → real updates.** Right now [mcp_middleware.rs](src/mcp_middleware/mcp_middleware.rs) answers `SubscribeResource` with the first version of the resource and forgets about it. Needed: keep a per-session list of subscribed URIs, add a `ResourceUpdated { uri }` variant to `McpSocketUpdateEvent`, and a public `McpMiddleware::notify_resource_updated(uri)` method that fans out only to the sessions subscribed to that URI.
- [ ] **`resources/unsubscribe`** — a spec method; not parsed in [mcp_payload.rs](src/mcp_middleware/mcp_payload.rs) yet.
- [ ] **`logging/setLevel` + `notifications/message`** — the server must accept the desired log level and send structured logs to the client. Needed: parsing the method, a `log_level` field in `McpSession`, a `McpSocketUpdateEvent::LogMessage { level, logger, data }` variant, and a public `McpMiddleware::log(level, logger, data)` API.
- [ ] **`notifications/progress`** — for long tool calls the client sends a `progressToken` in `params._meta.progressToken`. It is ignored now. Needed: pass the token to `McpToolCall::execute_tool_call` (via a context argument) and add a `ProgressReporter` that sends `notifications/progress` to the SSE stream of the calling session.
- [ ] **`completion/complete`** — auto-completion for prompt/resource arguments. Not implemented at all.
- [ ] **`roots/list` + `notifications/roots/list_changed`** — a client-side concept, but the server should be able to ask. Optional, if we want to initiate it from the server.
- [x] **`elicitation/create`** — implemented in 0.9.0. A tool implements `McpToolCallEx` and calls `ctx.elicit(message, schema, timeout)`. The client must declare `capabilities.elicitation: {}` at initialize, otherwise `elicit()` returns an error. Details: [stream_updates.rs](src/mcp_middleware/stream_updates.rs), [elicitations.rs](src/mcp_middleware/elicitations.rs), [tool_call_context.rs](src/mcp_middleware/tool_calls/tool_call_context.rs).
- [ ] **Sampling (`sampling/createMessage`)** — a server→client LLM request. Optional, for agentic scenarios.
- [ ] **`_meta` / `cursor` / `progressToken`** — currently stripped during parsing. They should pass straight through and be available to handlers.
- [ ] **JSON-RPC batch requests** — `try_parse` expects a single object, not an array. The spec requires batch support.
- [ ] **JSON-RPC error codes** — everything is currently mapped to `as_fatal_error` (HTTP 500). JSON-RPC errors `-32700`/`-32600`/`-32601`/`-32602`/`-32603` with the proper structure should be returned instead.
- [ ] **`ping` from server to client** — we can only answer it now; a liveness check of our own sessions needs `McpSocketUpdateEvent::Ping`.

## Tools / Prompts / Resources API

- [ ] **Automatic `tools/list_changed`** — currently fanned out only when the consumer explicitly calls `notify_tools_changed()`. Optional: trigger it from `register_tool_call` after `initialize` (if registration is dynamic at runtime).
- [x] **Resource templates (`resources/templates/list`)** — implemented in 0.10.0: `ResourceTemplateDefinition` + `McpResourceTemplateService`, registered via `register_resource_template`. Only RFC 6570 level 1 (`{name}`) is supported, with a variable being part of a single path segment; operators and modifiers are rejected at registration. Auto-completion of template arguments is the separate `completion/complete` item.
- [ ] **Tool annotations** — `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`. They should end up in `tools/list`. `ToolDefinition` does not expose them yet.
- [ ] **Tool `_meta` and `title`** — a separate human-readable title besides `name`/`description`.
- [ ] **Prompts with image/audio/embedded resource content** — currently `PromptExecutionResult.message: String`. The spec allows an array of content blocks of different types.
- [ ] **Multi-message prompts** — currently always a single user message. Should be a sequence of roles.
- [ ] **`structuredContent` validation against `outputSchema`** — `compile_execute_tool_call_response` currently writes the result as is, without checking it against the schema.

## Sessions and transport

- [ ] **Session TTL and eviction.** `last_access` is updated, but nothing cleans sessions up. A background task is needed that removes sessions older than N minutes.
- [ ] **SSE backpressure.** The channel is `mpsc::channel(32)`. With a slow client a broadcast silently drops messages — `let _ = sender.send(...)`. Decide: drop-oldest, drop-newest, or close the session.
- [ ] **`Last-Event-ID` / resumability.** The spec lets a client reconnect and catch up on missed events. Not supported yet.
- [ ] **`Mcp-Protocol-Version` header** — the client must send it with every request, and the server must validate compatibility.
- [ ] **CORS / Origin validation** — for browser clients; mandatory per the transport spec.
- [ ] **Authorization (OAuth 2.1)** — the MCP HTTP transport spec refers to a separate auth spec. Currently the only "auth" is the presence of `mcp-session-id`.

## Quality and infrastructure

- [x] **Errors reported outward, not only to stdout.** `McpMiddleware::register_error_hook(...)` + the `McpMiddlewareErrorHook` trait. The `McpMiddlewareError` event covers **all** of the middleware logic: tools (bad arguments / failed / unknown / result can not be serialized), prompts (failed / unknown), resources (unknown / read failed), protocol (payload could not be parsed / method not supported), sessions (missing header / unknown session on POST/GET/DELETE) and reading the request body. Each one carries the `session_id`. The hook is an **addition**, not a replacement: every event still goes to stderr as one line with a timestamp and the `McpMiddleware error:` marker. The exception is `SessionRejected`: it is debug info rather than an error (a client coming without a session or with an expired one is routine and happens all the time in production), so it reaches the console only in a debug build, with the `McpMiddleware debug:` marker — `cargo build --release` does not contain that code (`#[cfg(debug_assertions)]`); it always reaches the hook, and can be told apart with `err.is_debug_info()`. There are no `println!`/`eprintln!` calls of our own left in the runtime code. Details: [error_hook.rs](src/mcp_middleware/error_hook.rs).
- [ ] **Logging via `tracing`** — if structured output is needed instead of `eprintln!` in [error_hook.rs](src/mcp_middleware/error_hook.rs); it is a single point now, not scattered prints.
- [ ] **Tests.** Currently there is one `test_init_payload`. Needed:
  - parsing of all methods (including invalid JSON);
  - `tools/call` end to end through a test handler;
  - sessions: creation, validation, DELETE, expiry;
  - resources/list pagination with >100 resources;
  - SSE: subscription, broadcast, clear_sender on a disconnected client.
- [ ] **Doc comments on the public API** — `McpMiddleware`, traits, `notify_*`. Almost everything lacks `///` now.
- [ ] **README sync** — update it to the current API (re-exports, `notify_*`, DELETE).
- [ ] **CHANGELOG + versioning** — `Cargo.toml` is still `0.1.0`.
- [ ] **`Default` impl for `McpSessions` and `McpResources`** (clippy asks for it).
- [ ] **Clippy cleanup** — 40 warnings (needless_return, needless_borrow, etc.).

## Optional (nice-to-have)

- [ ] **Telemetry hooks** — tool call counters, latency, payload size.
- [ ] **Schema caching** — `get_input_params/get_output_params` call `JsonTypeDescription::get_description` every time. Could be cached after the first `tools/list`.
- [ ] **Configurable `PAGE_SIZE`** for resources pagination — currently the constant `100`.
- [ ] **WebSocket transport** — the spec mentions it as an alternative to HTTP+SSE.
