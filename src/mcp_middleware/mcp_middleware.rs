use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use my_http_server::{hyper::Method, *};
use rust_extensions::date_time::DateTimeAsMicroseconds;
use serde::{Serialize, de::DeserializeOwned};

use crate::mcp_middleware::{
    DynamicResourceExecutor, DynamicResources, InitializeMpcContract, McpConnectionInfo,
    McpElicitations, McpInputData, McpInputPayload, McpPromptService, McpPrompts,
    McpResourceService, McpResources, McpSessions, McpToolCallExWithInstruction,
    McpToolCallWithInstruction, McpToolCalls, PromptDefinition, PromptExecutor, RequestId,
    ResourceDefinition, ResourceExecutor, ResourceIcon, SESSION_HEADER, ToolCallContext,
    ToolCallExecutor, ToolCallExecutorEx, parse_elicitation_response,
};

use my_ai_agent::{ToolDefinition, json_schema::*};

pub struct McpMiddleware {
    mcp_path: &'static str,
    name: &'static str,
    version: &'static str,
    instructions: &'static str,
    sessions: Arc<McpSessions>,
    tool_calls: McpToolCalls,
    prompts: McpPrompts,
    resources: McpResources,
    /// Runtime-registered resources. Static resources go through
    /// `resources`; this registry serves URIs minted after `new()`
    /// (e.g. one resource per downloaded Telegram media item).
    dynamic_resources: Arc<tokio::sync::RwLock<DynamicResources>>,
    /// Registry of in-flight server→client `elicitation/create`
    /// requests. Tools opted into [`McpToolCallEx`] reach this through
    /// the [`ToolCallContext`] supplied at execute-time.
    elicitations: Arc<McpElicitations>,
    /// Sessions idle longer than this (and without a live SSE channel)
    /// are garbage-collected. See [`Self::with_session_idle_timeout`].
    session_idle_timeout: Duration,
    /// When on (the default), a non-`initialize` request carrying an
    /// unknown `mcp-session-id` adopts that id instead of getting a
    /// `404`. See [`Self::disabled_lazy_session_creation`].
    lazy_session_creation: bool,
    /// The GC task is started lazily on the first request, which is
    /// guaranteed to run inside the tokio runtime (unlike `new()`).
    gc_started: AtomicBool,
}

const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

impl McpMiddleware {
    pub fn new(
        mcp_path: &'static str,
        name: &'static str,
        version: &'static str,
        instructions: &'static str,
    ) -> Self {
        Self {
            mcp_path,
            name,
            version,
            instructions,
            sessions: Arc::new(McpSessions::new()),
            tool_calls: McpToolCalls::new(),
            prompts: McpPrompts::new(),
            resources: McpResources::new(),
            dynamic_resources: Arc::new(tokio::sync::RwLock::new(DynamicResources::new())),
            elicitations: Arc::new(McpElicitations::new()),
            session_idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT,
            lazy_session_creation: true,
            gc_started: AtomicBool::new(false),
        }
    }

    /// Overrides how long a session may stay idle (no requests, no live
    /// SSE stream) before the background GC drops it. Default: 30 min.
    pub fn with_session_idle_timeout(mut self, timeout: Duration) -> Self {
        self.session_idle_timeout = timeout;
        self
    }

    /// Registers the host hook for session lifecycle events — a session
    /// appeared (together with the request that created it) and a
    /// session is gone. Optional: without it nothing is fired and the
    /// request path stays exactly as it was. Only the first
    /// registration is kept.
    pub fn register_connection_info(
        &mut self,
        connection_info: Arc<dyn McpConnectionInfo + Send + Sync + 'static>,
    ) {
        self.sessions.set_connection_info(connection_info);
    }

    /// Turns lazy session creation off and restores the spec behavior:
    /// a non-`initialize` request whose `mcp-session-id` is unknown gets
    /// `404` so the client re-runs `initialize`. By default the id is
    /// adopted instead and the request is served, which keeps clients
    /// working across a server restart or a GC'd session.
    pub fn disabled_lazy_session_creation(mut self) -> Self {
        self.lazy_session_creation = false;
        self
    }

    /// Snapshot of the sessions the middleware currently holds, oldest
    /// `create` first (ties broken by id). Takes `&self`, so a host that
    /// handed its `Arc<McpMiddleware>` to `add_middleware` can keep
    /// polling it — pull costs nothing on the request path, unlike a
    /// third lifecycle event that would fire on every single request.
    ///
    /// `McpSession::last_access` is the exact clock the idle GC decides
    /// by, and it is refreshed by every request including `ping`.
    pub fn get_sessions(&self) -> Vec<super::McpSession> {
        self.sessions.get_sessions()
    }

    /// Pushes `notifications/resources/updated` for `uri` to every live
    /// session that subscribed to it via `resources/subscribe`. Call it
    /// whenever the content behind a resource changes.
    pub async fn notify_resource_updated(&self, uri: &str) {
        self.sessions.notify_resource_updated(uri).await;
    }

    pub async fn notify_tools_changed(&self) {
        self.sessions
            .broadcast(super::McpSocketUpdateEvent::ToolsListChanged)
            .await;
    }

    pub async fn notify_resources_changed(&self) {
        self.sessions
            .broadcast(super::McpSocketUpdateEvent::ResourcesListChanged)
            .await;
    }

    pub async fn notify_prompts_changed(&self) {
        self.sessions
            .broadcast(super::McpSocketUpdateEvent::PromptsListChanged)
            .await;
    }

    pub fn register_tool_call<
        InputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
        OutputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
        TMcpService: McpToolCallWithInstruction<InputData, OutputData>
            + Send
            + Sync
            + 'static
            + ToolDefinition,
    >(
        &mut self,
        service: Arc<TMcpService>,
    ) {
        let executor: ToolCallExecutor<InputData, OutputData> = ToolCallExecutor {
            fn_name: TMcpService::FUNC_NAME,
            description: TMcpService::DESCRIPTION,
            holder: service,
        };

        self.tool_calls.add(Arc::new(executor));
    }

    /// Same as [`Self::register_tool_call`] but for tools that need
    /// access to a [`ToolCallContext`] (server→client elicitation,
    /// session metadata, etc.). The tool implements [`McpToolCallEx`]
    /// or, when it wants to attach an `instruction` to the output,
    /// [`McpToolCallExWithInstruction`] directly.
    pub fn register_tool_call_with_context<
        InputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
        OutputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
        TMcpService: McpToolCallExWithInstruction<InputData, OutputData>
            + Send
            + Sync
            + 'static
            + ToolDefinition,
    >(
        &mut self,
        service: Arc<TMcpService>,
    ) {
        let executor: ToolCallExecutorEx<InputData, OutputData> = ToolCallExecutorEx {
            fn_name: TMcpService::FUNC_NAME,
            description: TMcpService::DESCRIPTION,
            holder: service,
        };

        self.tool_calls.add(Arc::new(executor));
    }

    pub fn register_prompt<
        TMcpPromptService: McpPromptService + Send + Sync + 'static + PromptDefinition,
    >(
        &mut self,
        service: Arc<TMcpPromptService>,
    ) {
        let executor = PromptExecutor {
            prompt_name: TMcpPromptService::PROMPT_NAME,
            description: TMcpPromptService::DESCRIPTION,
            argument_descriptions: TMcpPromptService::get_argument_descriptions(),
            holder: service,
        };

        self.prompts.add(Arc::new(executor));
    }

    pub fn register_resource<
        TMcpResourceService: McpResourceService + Send + Sync + 'static + ResourceDefinition,
    >(
        &mut self,
        service: Arc<TMcpResourceService>,
    ) {
        // Extract optional values before moving service - convert to owned values
        let title = service.get_title().map(|s| s.to_string());
        let size = service.get_size();
        let icons = service.get_icons();

        let executor = ResourceExecutor {
            resource_uri: TMcpResourceService::RESOURCE_URI,
            resource_name: TMcpResourceService::RESOURCE_NAME,
            description: TMcpResourceService::DESCRIPTION,
            mime_type: TMcpResourceService::MIME_TYPE,
            title,
            size,
            icons,
            holder: service,
        };

        self.resources.add(Arc::new(executor));
    }

    /// Register a resource minted at runtime. URI is whatever caller
    /// chooses (commonly `scheme://path/{id}`). Idempotent: registering
    /// the same URI twice overwrites the previous entry. Use
    /// [`Self::unregister_dynamic_resource`] for explicit removal and
    /// [`Self::notify_resources_changed`] to push the update to live
    /// MCP sessions.
    pub async fn register_dynamic_resource(
        &self,
        uri: String,
        name: String,
        description: String,
        mime_type: String,
        service: Arc<dyn McpResourceService + Send + Sync + 'static>,
    ) {
        self.register_dynamic_resource_full(
            uri, name, description, mime_type, None, None, Vec::new(), service,
        )
        .await
    }

    /// Same as [`Self::register_dynamic_resource`] but lets callers set
    /// the optional `title`, `size`, and `icons` metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_dynamic_resource_full(
        &self,
        uri: String,
        name: String,
        description: String,
        mime_type: String,
        title: Option<String>,
        size: Option<u64>,
        icons: Vec<ResourceIcon>,
        service: Arc<dyn McpResourceService + Send + Sync + 'static>,
    ) {
        let executor = DynamicResourceExecutor {
            resource_uri: uri,
            resource_name: name,
            description,
            mime_type,
            title,
            size,
            icons,
            holder: service,
        };
        let mut w = self.dynamic_resources.write().await;
        w.add(Arc::new(executor));
    }

    /// Drop a dynamic resource. Returns true if a resource with that
    /// URI was actually present. Callers that want clients to refresh
    /// their resource list should follow up with
    /// [`Self::notify_resources_changed`].
    pub async fn unregister_dynamic_resource(&self, uri: &str) -> bool {
        let mut w = self.dynamic_resources.write().await;
        w.remove(uri)
    }

    /// Shared by both the with-session and the without-session POST
    /// paths: `initialize` always mints a fresh session, even when the
    /// client sends a stale `mcp-session-id` header.
    async fn handle_initialize(
        &self,
        contract: InitializeMpcContract,
        now: DateTimeAsMicroseconds,
        id: &RequestId,
        ctx: Option<&mut HttpContext>,
    ) -> Result<HttpOkResult, HttpFailResult> {
        let protocol_version = super::mcp_output_contract::negotiate_protocol_version(
            contract.protocol_version.as_str(),
        )
        .to_string();

        let response = super::mcp_output_contract::compile_init_response(
            &self.name,
            &self.version,
            &self.instructions,
            protocol_version.as_str(),
            id,
            self.tool_calls.has_tools(),
            self.prompts.has_prompts(),
        );

        let supports_elicitation = contract.capabilities.elicitation.is_some();
        let session = self
            .sessions
            .generate_session(protocol_version, now, supports_elicitation);

        // A session appeared. `ctx` is None only when the middleware is
        // driven directly from a unit test; on the wire `initialize`
        // always arrives with its request context.
        if let Some(ctx) = ctx {
            self.sessions.notify_connected(&session, ctx).await;
        }

        send_response_as_stream(response, session.id.as_str(), now)
    }

    async fn handle_authorized_request(
        &self,
        session_id: &str,
        data: McpInputData,
        now: DateTimeAsMicroseconds,
        id: &RequestId,
        ctx: Option<&mut HttpContext>,
    ) -> Result<HttpOkResult, HttpFailResult> {
        match data {
            super::McpInputData::Initialize(contract) => {
                return self.handle_initialize(contract, now, id, ctx).await;
            }

            super::McpInputData::ResourcesList(params) => {
                let (mut list, next_cursor) =
                    self.resources.get_list(params.cursor.as_deref());

                // Append every dynamic resource. Pagination cursor is
                // driven by the static registry; once the static list
                // is exhausted (next_cursor = None) we surface the
                // dynamic ones on the same page.
                if next_cursor.is_none() {
                    let guard = self.dynamic_resources.read().await;
                    list.extend(guard.list());
                }

                let response = super::mcp_output_contract::compile_resources_list(
                    list,
                    id,
                    next_cursor.as_deref(),
                );

                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::ResourceTemplatesList => {
                // No URI-template support — an empty list keeps clients
                // that call this unconditionally (Inspector, Claude) happy.
                let response = super::mcp_output_contract::compile_resource_templates_list(id);
                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::ReadResource(params) => {
                let read_result = if self.resources.get(&params.uri).is_some() {
                    self.resources.read(&params.uri).await
                } else {
                    let guard = self.dynamic_resources.read().await;
                    if !guard.contains(&params.uri) {
                        return send_jsonrpc_error_as_stream(
                            super::mcp_output_contract::JSONRPC_RESOURCE_NOT_FOUND,
                            format!("Resource not found: {}", params.uri).as_str(),
                            id,
                            session_id,
                            now,
                        );
                    }
                    guard.read(&params.uri).await
                };

                match read_result {
                    Ok(response) => {
                        let response = super::mcp_output_contract::compile_read_resource_response(
                            response, id,
                        );
                        return send_response_as_stream(response, session_id, now);
                    }
                    Err(err) => {
                        eprintln!("Error reading resource with URI {}. Err: {}", params.uri, err);

                        return send_jsonrpc_error_as_stream(
                            super::mcp_output_contract::JSONRPC_INTERNAL_ERROR,
                            err.as_str(),
                            id,
                            session_id,
                            now,
                        );
                    }
                }
            }

            super::McpInputData::SubscribeResource(params) => {
                let known = self.resources.get(&params.uri).is_some()
                    || self.dynamic_resources.read().await.contains(&params.uri);

                if !known {
                    return send_jsonrpc_error_as_stream(
                        super::mcp_output_contract::JSONRPC_RESOURCE_NOT_FOUND,
                        format!("Resource not found: {}", params.uri).as_str(),
                        id,
                        session_id,
                        now,
                    );
                }

                self.sessions.subscribe(session_id, params.uri);

                // Per spec the subscribe response carries an empty result;
                // updates arrive later as `notifications/resources/updated`.
                let response = super::mcp_output_contract::compile_empty_result_response(id);
                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::UnsubscribeResource(params) => {
                // Idempotent: unsubscribing from an unknown URI is a no-op.
                self.sessions.unsubscribe(session_id, &params.uri);

                let response = super::mcp_output_contract::compile_empty_result_response(id);
                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::Ping => {
                let response = super::mcp_output_contract::compile_empty_result_response(id);
                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::ExecuteToolCall(params) => {
                // Unknown tool is a protocol-level error per spec, unlike
                // runtime failures which are reported in-band (isError).
                let Some(tool_call) = self.tool_calls.get(&params.name) else {
                    return send_jsonrpc_error_as_stream(
                        super::mcp_output_contract::JSONRPC_INVALID_PARAMS,
                        format!("Unknown tool: {}", params.name).as_str(),
                        id,
                        session_id,
                        now,
                    );
                };

                // serde(default) covers a missing `arguments` key; an
                // explicit `"arguments": null` still needs this guard.
                let arguments = if params.arguments.is_null() {
                    "{}".to_string()
                } else {
                    serde_json::to_string(&params.arguments).unwrap_or_else(|_| "{}".to_string())
                };

                let ctx = ToolCallContext {
                    session_id: session_id.to_string(),
                    supports_elicitation: self
                        .sessions
                        .session_supports_elicitation(session_id),
                    elicitations: self.elicitations.clone(),
                    sessions: self.sessions.clone(),
                };

                // The SSE response stream opens immediately and emits
                // keepalive comments while the tool runs, so proxies do
                // not cut long calls (elicitation can wait on a human
                // for minutes). If the client disconnects mid-call the
                // keepalive send fails and the tool future is dropped,
                // i.e. the call is cancelled — half-done side effects
                // are the tool's responsibility.
                let (http_output, mut producer) = HttpOutput::as_stream(32);

                let id = id.clone();
                let tool_name = params.name;

                tokio::spawn(async move {
                    let execute = tool_call.execute(arguments.as_str(), ctx);
                    tokio::pin!(execute);

                    let mut keepalive = tokio::time::interval(super::KEEPALIVE_INTERVAL);
                    // interval()'s first tick fires immediately — skip it.
                    keepalive.tick().await;

                    loop {
                        tokio::select! {
                            result = &mut execute => {
                                let response = match result {
                                    Ok(executed) => {
                                        super::mcp_output_contract::compile_execute_tool_call_response(
                                            executed.structured_json,
                                            executed.instruction,
                                            &id,
                                            false,
                                        )
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "Error executing {} with params {}. Err: {}",
                                            tool_name, arguments, err
                                        );
                                        super::mcp_output_contract::compile_execute_tool_call_response(
                                            err, None, &id, true,
                                        )
                                    }
                                };

                                let _ = producer.send(response.into_bytes()).await;
                                return;
                            }
                            _ = keepalive.tick() => {
                                if producer.send(b": keepalive\n\n".to_vec()).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });

                return http_output
                    .with_header(SESSION_HEADER, session_id)
                    .with_header("cache-control", "no-cache")
                    .with_header("content-type", "text/event-stream")
                    .with_header("date", now.to_rfc7231())
                    .get_result();
            }

            super::McpInputData::ToolsList => {
                let list = self.tool_calls.get_list().await;
                let response = super::mcp_output_contract::compile_tool_calls(list, id);

                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::PromptsList => {
                let list = self.prompts.get_list();
                let response = super::mcp_output_contract::compile_prompts_list(list, id);

                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::GetPrompt(params) => {
                let arguments = match params.arguments {
                    Some(args) => args,
                    None => Default::default(),
                };

                // Unknown prompt name → protocol-level Invalid params.
                let Some(prompt) = self.prompts.get(&params.name) else {
                    return send_jsonrpc_error_as_stream(
                        super::mcp_output_contract::JSONRPC_INVALID_PARAMS,
                        format!("Unknown prompt: {}", params.name).as_str(),
                        id,
                        session_id,
                        now,
                    );
                };

                match prompt.execute(&arguments).await {
                    Ok(response) => {
                        let response =
                            super::mcp_output_contract::compile_get_prompt_response(response, id);
                        return send_response_as_stream(response, session_id, now);
                    }
                    Err(err) => {
                        eprintln!(
                            "Error executing prompt {} with params {:?}. Err: {}",
                            params.name, arguments, err
                        );

                        return send_jsonrpc_error_as_stream(
                            super::mcp_output_contract::JSONRPC_INTERNAL_ERROR,
                            err.as_str(),
                            id,
                            session_id,
                            now,
                        );
                    }
                }
            }

            super::McpInputData::NotificationsInitialize => {
                return accepted_response(now);
            }

            super::McpInputData::Notification { method: _ } => {
                // Per the Streamable HTTP transport every accepted
                // notification gets 202; ones we have no handler for
                // (notifications/cancelled, roots/list_changed, ...)
                // are simply ignored.
                return accepted_response(now);
            }

            super::McpInputData::ServerResponse {
                result_json,
                error_json,
            } => {
                let response = parse_elicitation_response(
                    result_json.as_deref(),
                    error_json.as_deref(),
                );
                if let Some(request_id) = id.as_int() {
                    self.elicitations.resolve(request_id, response);
                }
                return accepted_response(now);
            }

            super::McpInputData::Other { method, data } => {
                eprintln!("Unsupported MCP method: {}. Data: `{}`", method, data);

                // Requests (id present) get a JSON-RPC error; id-less
                // inputs are notifications by definition → 202.
                if id.is_null() {
                    return accepted_response(now);
                }

                return send_jsonrpc_error_as_stream(
                    super::mcp_output_contract::JSONRPC_METHOD_NOT_FOUND,
                    format!("Method not found: {}", method).as_str(),
                    id,
                    session_id,
                    now,
                );
            }
        }
    }

    async fn handle_post_request(
        &self,
        session_id: Option<&str>,
        body: &[u8],
        mut ctx: Option<&mut HttpContext>,
    ) -> Result<HttpOkResult, HttpFailResult> {
        let now = DateTimeAsMicroseconds::now();

        let payload = match super::McpInputPayload::try_parse(body) {
            Ok(payload) => payload,
            Err(err) => {
                // Malformed JSON-RPC → HTTP 400 with a standard Parse
                // error body (no SSE framing on plain HTTP errors).
                let body = super::mcp_output_contract::compile_jsonrpc_error_body(
                    super::mcp_output_contract::JSONRPC_PARSE_ERROR,
                    format!("Parse error: {}", err).as_str(),
                    &RequestId::Null,
                );
                return HttpOutput::from_builder()
                    .set_content(body.into_bytes())
                    .set_content_type(WebContentType::Json)
                    .set_status_code(400)
                    .add_header("date", now.to_rfc7231())
                    .into_ok_result(false);
            }
        };

        let McpInputPayload { id, data, .. } = payload;

        // `initialize` is valid both with and without a session header —
        // a stale header must not block a client from re-initializing.
        if let super::McpInputData::Initialize(contract) = data {
            return self.handle_initialize(contract, now, &id, ctx).await;
        }

        let Some(session_id) = session_id else {
            // Spec: every non-initialize request must carry the session
            // header once the server has issued one.
            return Err(HttpFailResult::as_validation_error(
                "Missing mcp-session-id header",
            ));
        };

        if !self
            .sessions
            .check_session_and_update_last_used(session_id, now)
        {
            if !self.lazy_session_creation {
                // Spec: 404 signals the session is gone and the client
                // should start over with a new `initialize`.
                return Err(HttpFailResult::as_not_found("Unknown MCP session", false));
            }

            // Lazy session creation: adopt the id the client already
            // holds (server restart, GC'd session) and serve the request
            // as if `initialize` had just run — latest protocol version,
            // no elicitation support until the client says otherwise.
            let created = self.sessions.ensure_session_with_id(
                session_id,
                super::mcp_output_contract::latest_protocol_version().to_string(),
                now,
                false,
            );

            // Adopting an id is a session appearing just as much as
            // `initialize` is — the host must hear about it.
            if let Some(session) = created {
                if let Some(ctx) = ctx.as_deref_mut() {
                    self.sessions.notify_connected(&session, ctx).await;
                }
            }
        }

        self.handle_authorized_request(session_id, data, now, &id, ctx)
            .await
    }
}

fn accepted_response(now: DateTimeAsMicroseconds) -> Result<HttpOkResult, HttpFailResult> {
    HttpOutput::from_builder()
        .add_header("date", now.to_rfc7231())
        .set_status_code(202)
        .into_ok_result(false)
}

fn send_jsonrpc_error_as_stream(
    code: i64,
    message: &str,
    id: &RequestId,
    session_id: &str,
    now: DateTimeAsMicroseconds,
) -> Result<HttpOkResult, HttpFailResult> {
    let response = super::mcp_output_contract::compile_jsonrpc_error(code, message, id);
    send_response_as_stream(response, session_id, now)
}

fn send_response_as_stream(
    response: String,
    session_id: &str,
    now: DateTimeAsMicroseconds,
) -> Result<HttpOkResult, HttpFailResult> {
    let (http_output, mut producer) = HttpOutput::as_stream(1024);
    tokio::spawn(async move {
        let payload = response.into_bytes();
        // Client may disconnect before reading the response — nothing to do.
        let _ = producer.send(payload).await;
    });

    http_output
        .with_header(SESSION_HEADER, session_id)
        .with_header("cache-control", "no-cache")
        .with_header("content-type", "text/event-stream")
        .with_header("date", now.to_rfc7231())
        .get_result()
}

#[async_trait::async_trait]
impl HttpServerMiddleware for McpMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        if !ctx
            .request
            .get_path()
            .equals_to_case_insensitive(self.mcp_path)
        {
            return None;
        }

        // Lazy GC start: handle_request always runs inside the tokio
        // runtime, which `new()` can not guarantee.
        if !self.gc_started.swap(true, Ordering::Relaxed) {
            super::spawn_session_gc(Arc::downgrade(&self.sessions), self.session_idle_timeout);
        }

        let session_id = ctx
            .request
            .get_headers()
            .try_get_case_sensitive(SESSION_HEADER)
            .and_then(|itm| itm.as_str().ok().map(|s| s.to_string()));

        match ctx.request.method {
            Method::GET => {
                let Some(session_id) = session_id else {
                    return Some(
                        HttpFailResult::as_validation_error("Missing mcp-session-id header")
                            .into_err(),
                    );
                };

                let now = DateTimeAsMicroseconds::now();

                if let Some(receiver) = self
                    .sessions
                    .subscribe_to_notifications(session_id.as_str(), now)
                {
                    let (stream, producer) = HttpOutput::as_stream(32);
                    tokio::spawn(super::stream_updates(
                        producer,
                        receiver,
                        self.sessions.clone(),
                        session_id.clone(),
                    ));

                    return Some(
                        stream
                            .with_header("content-type", "text/event-stream")
                            .with_header("cache-control", "no-cache")
                            .with_header("date", now.to_rfc7231())
                            .get_result(),
                    );
                }

                return Some(
                    HttpFailResult::as_not_found("Unknown MCP session", false).into_err(),
                );
            }
            Method::POST => {
                // A registered connection-info hook is handed the whole
                // HttpContext, which can not be borrowed while the
                // reference returned by `get_body()` is alive — so for
                // listening hosts the body is copied once. With no hook
                // the zero-copy path is untouched.
                if self.sessions.has_connection_info() {
                    let body = match ctx.request.get_body().await {
                        Ok(body) => body.as_slice().to_vec(),
                        Err(err) => {
                            return Some(Err(err));
                        }
                    };

                    let result = self
                        .handle_post_request(session_id.as_deref(), body.as_slice(), Some(ctx))
                        .await;
                    return Some(result);
                }

                let body = match ctx.request.get_body().await {
                    Ok(body) => body,
                    Err(err) => {
                        return Some(Err(err));
                    }
                };

                let result = self
                    .handle_post_request(session_id.as_deref(), body.as_slice(), None)
                    .await;
                return Some(result);
            }
            Method::DELETE => {
                let Some(session_id) = session_id else {
                    return Some(
                        HttpFailResult::as_validation_error("Missing mcp-session-id header")
                            .into_err(),
                    );
                };

                let removed = self.sessions.delete_session(session_id.as_str()).await;

                if !removed {
                    return Some(
                        HttpFailResult::as_not_found("Unknown MCP session", false).into_err(),
                    );
                }

                let now = DateTimeAsMicroseconds::now();
                return Some(
                    HttpOutput::from_builder()
                        .add_header("date", now.to_rfc7231())
                        .set_status_code(204)
                        .into_ok_result(false),
                );
            }
            _ => {}
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolDefinition;
    use crate::mcp_middleware::{McpSession, McpToolCall};
    use my_ai_agent::json_schema::JsonTypeDescription;

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct EchoInput {
        #[serde(default)]
        text: Option<String>,
    }

    #[async_trait::async_trait]
    impl JsonTypeDescription for EchoInput {
        async fn get_description(
            _has_default: bool,
            _with_enum: Option<Vec<rust_extensions::StrOrString<'static>>>,
            _output: bool,
        ) -> my_ai_agent::my_json::json_writer::JsonObjectWriter {
            my_ai_agent::my_json::json_writer::JsonObjectWriter::new().write("type", "object")
        }
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct EchoOutput {
        echoed: String,
    }

    #[async_trait::async_trait]
    impl JsonTypeDescription for EchoOutput {
        async fn get_description(
            _has_default: bool,
            _with_enum: Option<Vec<rust_extensions::StrOrString<'static>>>,
            _output: bool,
        ) -> my_ai_agent::my_json::json_writer::JsonObjectWriter {
            my_ai_agent::my_json::json_writer::JsonObjectWriter::new().write("type", "object")
        }
    }

    struct EchoTool;

    impl ToolDefinition for EchoTool {
        const FUNC_NAME: &'static str = "echo";
        const DESCRIPTION: &'static str = "Echoes the input back";
    }

    #[async_trait::async_trait]
    impl McpToolCall<EchoInput, EchoOutput> for EchoTool {
        async fn execute_tool_call(&self, model: EchoInput) -> Result<EchoOutput, String> {
            Ok(EchoOutput {
                echoed: model.text.unwrap_or_default(),
            })
        }
    }

    fn middleware_with_echo_tool() -> McpMiddleware {
        let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
        mcp.register_tool_call(Arc::new(EchoTool));
        mcp
    }

    /// Records the lifecycle events the middleware fires. `on_connected`
    /// needs a real `HttpContext`, which only a real request can produce
    /// — it is covered by `tests/session_lifecycle.rs`.
    #[derive(Default)]
    struct RecordingConnectionInfo {
        disconnected: parking_lot::Mutex<Vec<String>>,
    }

    impl RecordingConnectionInfo {
        fn disconnected(&self) -> Vec<String> {
            self.disconnected.lock().clone()
        }
    }

    #[async_trait::async_trait]
    impl McpConnectionInfo for RecordingConnectionInfo {
        async fn on_connected(&self, _session: &McpSession, _ctx: &mut HttpContext) {}

        async fn on_disconnected(&self, session: &McpSession) {
            self.disconnected.lock().push(session.id.clone());
        }
    }

    fn middleware_with_recorder() -> (McpMiddleware, Arc<RecordingConnectionInfo>) {
        let recorder = Arc::new(RecordingConnectionInfo::default());
        let mut mcp = middleware_with_echo_tool();
        mcp.register_connection_info(recorder.clone());
        (mcp, recorder)
    }

    /// Drains an SSE (`HttpOutput::Raw`) response: returns
    /// (status, body, mcp-session-id header).
    async fn read_sse_response(
        result: Result<HttpOkResult, HttpFailResult>,
    ) -> (u16, String, Option<String>) {
        let ok = result.expect("expected Ok result");
        match ok.output {
            HttpOutput::Raw(response) => {
                let status = response.status().as_u16();
                let session_id = response
                    .headers()
                    .get(SESSION_HEADER)
                    .map(|v| v.to_str().unwrap().to_string());
                let collected = http_body_util::BodyExt::collect(response.into_body())
                    .await
                    .expect("body collected");
                let body = String::from_utf8(collected.to_bytes().to_vec()).unwrap();
                (status, body, session_id)
            }
            other => panic!("expected Raw stream output, got {:?}", other),
        }
    }

    async fn initialize_session(mcp: &McpMiddleware) -> String {
        let body = br#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        let (status, _, session_id) = read_sse_response(result).await;
        assert_eq!(status, 200);
        session_id.expect("initialize must return mcp-session-id header")
    }

    #[tokio::test]
    async fn initialize_returns_session_and_capabilities() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        let (status, body, session_id) = read_sse_response(result).await;

        assert_eq!(status, 200);
        assert!(session_id.is_some());
        assert!(body.contains(r#""protocolVersion":"2025-06-18""#));
        assert!(body.contains(r#""subscribe":true"#));
        assert!(body.contains(r#""tools""#));
    }

    #[tokio::test]
    async fn initialize_with_unknown_version_falls_back_to_latest() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"1999-01-01","capabilities":{}}}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""protocolVersion":"2025-11-25""#));
    }

    #[tokio::test]
    async fn initialize_with_stale_session_header_mints_new_session() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
        let result = mcp.handle_post_request(Some("stale-session"), body, None).await;
        let (status, _, session_id) = read_sse_response(result).await;

        assert_eq!(status, 200);
        assert!(session_id.is_some());
        assert_ne!(session_id.unwrap(), "stale-session");
    }

    #[tokio::test]
    async fn notifications_are_accepted_with_202() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        for body in [
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.as_slice(),
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#
                .as_slice(),
        ] {
            let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
            let ok = result.expect("notification must be accepted");
            assert_eq!(ok.output.get_status_code(), 202);
        }
    }

    #[tokio::test]
    async fn unknown_method_with_id_gets_method_not_found() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"logging/setLevel","id":7,"params":{"level":"debug"}}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (status, body, _) = read_sse_response(result).await;

        assert_eq!(status, 200);
        assert!(body.contains(r#""code":-32601"#));
        assert!(body.contains(r#""id":7"#));
    }

    #[tokio::test]
    async fn missing_session_header_is_400() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        let Err(err) = result else {
            panic!("must be rejected");
        };
        assert_eq!(err.output.get_status_code(), 400);
    }

    #[tokio::test]
    async fn unknown_session_is_404_when_lazy_creation_is_disabled() {
        let mcp = middleware_with_echo_tool().disabled_lazy_session_creation();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp.handle_post_request(Some("no-such-session"), body, None).await;
        let Err(err) = result else {
            panic!("must be rejected");
        };
        assert_eq!(err.output.get_status_code(), 404);
    }

    #[tokio::test]
    async fn unknown_session_is_adopted_by_default() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp.handle_post_request(Some("client-owned-id"), body, None).await;
        let (status, body, session_id) = read_sse_response(result).await;

        assert_eq!(status, 200);
        // The client-supplied id is kept as-is, not replaced by a new one.
        assert_eq!(session_id.as_deref(), Some("client-owned-id"));
        assert!(body.contains(r#""echo""#));

        // The session is now a regular one: it survives to the next request.
        let now = DateTimeAsMicroseconds::now();
        assert!(
            mcp.sessions
                .check_session_and_update_last_used("client-owned-id", now)
        );
    }

    #[tokio::test]
    async fn missing_session_header_stays_400_with_lazy_creation() {
        let mcp = middleware_with_echo_tool();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        let Err(err) = result else {
            panic!("must be rejected");
        };
        assert_eq!(err.output.get_status_code(), 400);
    }

    #[tokio::test]
    async fn malformed_body_is_400_with_parse_error() {
        let mcp = middleware_with_echo_tool();

        let result = mcp.handle_post_request(None, b"this is not json", None).await;
        let ok = result.expect("400 is returned as ok-result with JSON body");
        match ok.output {
            HttpOutput::Content {
                status_code,
                content,
                ..
            } => {
                assert_eq!(status_code, 400);
                let body = String::from_utf8(content).unwrap();
                assert!(body.contains(r#""code":-32700"#));
                assert!(body.contains(r#""id":null"#));
            }
            other => panic!("expected Content output, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn unknown_tool_gets_invalid_params() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"nope","arguments":{}}}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""code":-32602"#));
        assert!(body.contains("Unknown tool: nope"));
    }

    #[tokio::test]
    async fn tool_call_without_arguments_succeeds_and_streams_result() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":6,"params":{"name":"echo"}}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (status, body, _) = read_sse_response(result).await;

        assert_eq!(status, 200);
        assert!(body.contains(r#""isError":false"#));
        assert!(body.contains(r#""echoed":"""#));
    }

    #[tokio::test]
    async fn resource_templates_list_returns_empty_array() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"resources/templates/list","id":8}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""resourceTemplates":[]"#));
    }

    #[tokio::test]
    async fn subscribe_unknown_resource_is_resource_not_found() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"resources/subscribe","id":9,"params":{"uri":"res://missing"}}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""code":-32002"#));
    }

    #[tokio::test]
    async fn delete_reports_the_session_as_gone_exactly_once() {
        let (mcp, recorder) = middleware_with_recorder();
        let session_id = initialize_session(&mcp).await;

        assert!(mcp.sessions.delete_session(session_id.as_str()).await);
        assert_eq!(recorder.disconnected(), vec![session_id.clone()]);

        // The session is gone — a repeated DELETE must not fire again.
        assert!(!mcp.sessions.delete_session(session_id.as_str()).await);
        assert_eq!(recorder.disconnected(), vec![session_id]);
    }

    #[tokio::test]
    async fn deleting_an_unknown_session_reports_nothing() {
        let (mcp, recorder) = middleware_with_recorder();

        assert!(!mcp.sessions.delete_session("never-existed").await);
        assert!(recorder.disconnected().is_empty());
    }

    #[tokio::test]
    async fn gc_reports_every_session_it_collects() {
        let (mcp, recorder) = middleware_with_recorder();
        let expired = initialize_session(&mcp).await;
        let fresh = initialize_session(&mcp).await;

        let mut later = DateTimeAsMicroseconds::now();
        later.add_seconds(3600);

        let removed = mcp
            .sessions
            .remove_idle_sessions(later, Duration::from_secs(1800))
            .await;
        assert_eq!(removed, 2);

        let mut disconnected = recorder.disconnected();
        disconnected.sort();
        let mut expected = vec![expired, fresh];
        expected.sort();
        assert_eq!(disconnected, expected);

        // Nothing is left to collect, so a second sweep stays quiet.
        let removed = mcp
            .sessions
            .remove_idle_sessions(later, Duration::from_secs(1800))
            .await;
        assert_eq!(removed, 0);
        assert_eq!(recorder.disconnected().len(), 2);
    }

    #[tokio::test]
    async fn lazily_created_session_is_reported_as_gone_too() {
        let (mcp, recorder) = middleware_with_recorder();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp
            .handle_post_request(Some("client-owned-id"), body, None)
            .await;
        let (status, _, _) = read_sse_response(result).await;
        assert_eq!(status, 200);

        assert!(mcp.sessions.delete_session("client-owned-id").await);
        assert_eq!(recorder.disconnected(), vec!["client-owned-id".to_string()]);
    }

    #[tokio::test]
    async fn session_removal_works_without_a_registered_hook() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        assert!(mcp.sessions.delete_session(session_id.as_str()).await);

        let other = initialize_session(&mcp).await;
        let mut later = DateTimeAsMicroseconds::now();
        later.add_seconds(3600);
        assert_eq!(
            mcp.sessions
                .remove_idle_sessions(later, Duration::from_secs(1800))
                .await,
            1
        );
        assert!(!mcp
            .sessions
            .check_session_and_update_last_used(other.as_str(), later));
    }

    /// The one live session, or a panic — every caller below runs
    /// against a middleware that has exactly one.
    fn only_session(mcp: &McpMiddleware) -> McpSession {
        let mut sessions = mcp.get_sessions();
        assert_eq!(sessions.len(), 1);
        sessions.remove(0)
    }

    #[tokio::test]
    async fn fresh_session_starts_with_last_access_at_create_time() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let session = only_session(&mcp);
        assert_eq!(session.id, session_id);
        assert_eq!(
            session.last_access.get_unix_microseconds(),
            session.create.unix_microseconds
        );
    }

    #[tokio::test]
    async fn a_regular_request_moves_last_access() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;
        let before = only_session(&mcp).last_access.get_unix_microseconds();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":2}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (status, _, _) = read_sse_response(result).await;
        assert_eq!(status, 200);

        let session = only_session(&mcp);
        assert!(session.last_access.get_unix_microseconds() > before);
        // ...and `create` stays put, so the two are now distinguishable.
        assert!(session.last_access.as_date_time() > session.create);
    }

    /// The whole point of exposing `last_access`: a client that only
    /// pings is alive, and the host must be able to see that.
    #[tokio::test]
    async fn ping_moves_last_access() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;
        let before = only_session(&mcp).last_access.get_unix_microseconds();

        let body = br#"{"jsonrpc":"2.0","method":"ping","id":3}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (status, _, _) = read_sse_response(result).await;
        assert_eq!(status, 200);

        assert!(only_session(&mcp).last_access.get_unix_microseconds() > before);
    }

    #[tokio::test]
    async fn get_sessions_reflects_deleted_and_collected_sessions() {
        let mcp = middleware_with_echo_tool();
        assert!(mcp.get_sessions().is_empty());

        let deleted = initialize_session(&mcp).await;
        let collected = initialize_session(&mcp).await;
        assert_eq!(mcp.get_sessions().len(), 2);

        assert!(mcp.sessions.delete_session(deleted.as_str()).await);
        let ids: Vec<String> = mcp
            .get_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(ids, vec![collected]);

        let mut later = DateTimeAsMicroseconds::now();
        later.add_seconds(3600);
        assert_eq!(
            mcp.sessions
                .remove_idle_sessions(later, Duration::from_secs(1800))
                .await,
            1
        );
        assert!(mcp.get_sessions().is_empty());
    }

    #[tokio::test]
    async fn string_request_id_is_echoed_in_response() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":"req-42"}"#;
        let result = mcp.handle_post_request(Some(session_id.as_str()), body, None).await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""id":"req-42""#));
    }
}
