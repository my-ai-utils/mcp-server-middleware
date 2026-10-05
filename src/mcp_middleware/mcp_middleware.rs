use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use my_http_server::{hyper::Method, *};
use rust_extensions::date_time::DateTimeAsMicroseconds;
use serde::{Serialize, de::DeserializeOwned};

use crate::mcp_middleware::{
    DynamicResourceExecutor, DynamicResources, InitializeMpcContract, McpConnectionInfo,
    McpElicitations, McpErrorReporter, McpInputData, McpInputPayload, McpMiddlewareError,
    McpMiddlewareErrorHook, McpPromptService, McpPrompts, McpResourceService,
    McpResourceTemplateService, McpResourceTemplates, McpResources, McpSessions,
    McpToolCallExWithInstruction, McpToolCallWithInstruction, McpToolCalls, PromptDefinition,
    PromptExecutor, RequestId, ResourceDefinition, ResourceExecutor, ResourceIcon,
    ResourceTemplateDefinition, ResourceTemplateExecutor, ResourceTemplateReadError,
    SESSION_HEADER, ToolCallContext, ToolCallExecutor, ToolCallExecutorEx, UriTemplate,
    parse_elicitation_response,
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
    /// URI templates (`docs://lib/{topic}`): one handler serves every URI
    /// that matches. Consulted after the static and the dynamic registries.
    resource_templates: McpResourceTemplates,
    /// Runtime-registered resources. Static resources go through
    /// `resources`; this registry serves URIs minted after `new()`
    /// (e.g. one resource per downloaded Telegram media item).
    dynamic_resources: Arc<tokio::sync::RwLock<DynamicResources>>,
    /// Registry of in-flight server→client `elicitation/create`
    /// requests. Tools opted into [`McpToolCallEx`] reach this through
    /// the [`ToolCallContext`] supplied at execute-time.
    elicitations: Arc<McpElicitations>,
    /// Optional host hook for the errors this middleware would otherwise
    /// only print. See [`Self::register_error_hook`].
    errors: Arc<McpErrorReporter>,
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

/// The two session-level refusals, spelled once: they are both what the
/// client is told and what [`McpMiddlewareError::SessionRejected`]
/// reports, and a host matching on the text would not enjoy a typo.
const MISSING_SESSION_HEADER: &str = "Missing mcp-session-id header";
const UNKNOWN_SESSION: &str = "Unknown MCP session";

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
            resource_templates: McpResourceTemplates::new(),
            dynamic_resources: Arc::new(tokio::sync::RwLock::new(DynamicResources::new())),
            elicitations: Arc::new(McpElicitations::new()),
            errors: Arc::new(McpErrorReporter::new()),
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

    /// Registers the host hook for the errors the middleware runs into:
    /// arguments that do not match a tool's `inputSchema`, a tool or
    /// prompt that returned `Err`, an unknown tool name, an unparsable
    /// JSON-RPC payload. Optional; only the first registration is kept.
    ///
    /// The hook is additional, not a replacement — every event is written
    /// to stderr either way, prefixed with the timestamp and
    /// `McpMiddleware`.
    ///
    /// The hook is awaited on the request path, so keep it cheap. It can
    /// not change the response: see [`McpMiddlewareErrorHook`].
    pub fn register_error_hook(
        &mut self,
        hook: Arc<dyn McpMiddlewareErrorHook + Send + Sync + 'static>,
    ) {
        self.errors.set_hook(hook);
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

    /// Registers a resource template: every URI that matches its
    /// `URI_TEMPLATE` is read through the one service, which gets the
    /// template's variables. Listed in `resources/templates/list`;
    /// `resources/read` falls back to the templates when neither a static
    /// nor a dynamic resource has the exact URI.
    ///
    /// Panics when `URI_TEMPLATE` is not a level-1 template - see
    /// [`ResourceTemplateDefinition::URI_TEMPLATE`]. Registering the same
    /// template twice replaces the previous entry.
    pub fn register_resource_template<
        TMcpResourceTemplateService: McpResourceTemplateService
            + Send
            + Sync
            + 'static
            + ResourceTemplateDefinition,
    >(
        &mut self,
        service: Arc<TMcpResourceTemplateService>,
    ) {
        let uri_template = TMcpResourceTemplateService::URI_TEMPLATE;

        let template = UriTemplate::parse(uri_template).unwrap_or_else(|err| {
            panic!(
                "Resource template `{}` is not supported: {}",
                uri_template, err
            )
        });

        let executor = ResourceTemplateExecutor {
            uri_template,
            template_name: TMcpResourceTemplateService::TEMPLATE_NAME,
            description: TMcpResourceTemplateService::DESCRIPTION,
            mime_type: TMcpResourceTemplateService::MIME_TYPE,
            title: service.get_title().map(|s| s.to_string()),
            icons: service.get_icons(),
            template,
            holder: service,
        };

        self.resource_templates.add(executor);
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
                let response = super::mcp_output_contract::compile_resource_templates_list(
                    self.resource_templates.get_list(),
                    id,
                );
                return send_response_as_stream(response, session_id, now);
            }

            super::McpInputData::ReadResource(params) => {
                let read_result = if self.resources.get(&params.uri).is_some() {
                    self.resources.read(&params.uri).await
                } else {
                    let guard = self.dynamic_resources.read().await;
                    if !guard.contains(&params.uri) {
                        drop(guard);
                        return self
                            .read_templated_resource(session_id, params.uri.as_str(), now, id)
                            .await;
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
                        self.errors
                            .report(McpMiddlewareError::ResourceRead {
                                session_id,
                                uri: params.uri.as_str(),
                                error: err.as_str(),
                            })
                            .await;

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
                // A templated URI is accepted by its shape alone - the
                // template's handler is not asked whether it exists.
                let known = self.resources.get(&params.uri).is_some()
                    || self.dynamic_resources.read().await.contains(&params.uri)
                    || self.resource_templates.find(&params.uri).is_some();

                if !known {
                    self.errors
                        .report(McpMiddlewareError::ResourceNotFound {
                            session_id,
                            method: "resources/subscribe",
                            uri: params.uri.as_str(),
                        })
                        .await;

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
                // serde(default) covers a missing `arguments` key; an
                // explicit `"arguments": null` still needs this guard.
                // Computed before the tool lookup so an unknown tool can
                // be reported together with what it was called with.
                let arguments = if params.arguments.is_null() {
                    "{}".to_string()
                } else {
                    serde_json::to_string(&params.arguments).unwrap_or_else(|_| "{}".to_string())
                };

                // Unknown tool is a protocol-level error per spec, unlike
                // runtime failures which are reported in-band (isError).
                let Some(tool_call) = self.tool_calls.get(&params.name) else {
                    self.errors
                        .report(McpMiddlewareError::ToolNotFound {
                            session_id,
                            tool_name: params.name.as_str(),
                            arguments: arguments.as_str(),
                        })
                        .await;

                    return send_jsonrpc_error_as_stream(
                        super::mcp_output_contract::JSONRPC_INVALID_PARAMS,
                        format!("Unknown tool: {}", params.name).as_str(),
                        id,
                        session_id,
                        now,
                    );
                };

                let ctx = ToolCallContext {
                    session_id: session_id.to_string(),
                    supports_elicitation: self
                        .sessions
                        .session_supports_elicitation(session_id),
                    elicitations: self.elicitations.clone(),
                    sessions: self.sessions.clone(),
                    errors: self.errors.clone(),
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
                // The tool runs in a detached task, so the reporter and
                // the session id have to be carried into it by value.
                let errors = self.errors.clone();
                let error_session_id = session_id.to_string();

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
                                        errors
                                            .report(McpMiddlewareError::ToolExecution {
                                                session_id: error_session_id.as_str(),
                                                tool_name: tool_name.as_str(),
                                                arguments: arguments.as_str(),
                                                error: err.as_str(),
                                            })
                                            .await;

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
                    self.errors
                        .report(McpMiddlewareError::PromptNotFound {
                            session_id,
                            prompt_name: params.name.as_str(),
                            arguments: &arguments,
                        })
                        .await;

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
                        self.errors
                            .report(McpMiddlewareError::PromptExecution {
                                session_id,
                                prompt_name: params.name.as_str(),
                                arguments: &arguments,
                                error: err.as_str(),
                            })
                            .await;

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
                self.errors
                    .report(McpMiddlewareError::MethodNotFound {
                        session_id,
                        method: method.as_str(),
                        payload: data.as_str(),
                    })
                    .await;

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

    /// `resources/read` of a URI that is neither a static nor a dynamic
    /// resource: the last chance is a template that matches it.
    async fn read_templated_resource(
        &self,
        session_id: &str,
        uri: &str,
        now: DateTimeAsMicroseconds,
        id: &RequestId,
    ) -> Result<HttpOkResult, HttpFailResult> {
        let read_result = match self.resource_templates.find(uri) {
            Some((template, variables)) => template.holder.read_resource(uri, &variables).await,
            None => Err(ResourceTemplateReadError::NotFound(format!(
                "Resource not found: {}",
                uri
            ))),
        };

        match read_result {
            Ok(response) => {
                let response =
                    super::mcp_output_contract::compile_read_resource_response(response, id);
                send_response_as_stream(response, session_id, now)
            }
            Err(ResourceTemplateReadError::NotFound(message)) => {
                self.errors
                    .report(McpMiddlewareError::ResourceNotFound {
                        session_id,
                        method: "resources/read",
                        uri,
                    })
                    .await;

                send_jsonrpc_error_as_stream(
                    super::mcp_output_contract::JSONRPC_RESOURCE_NOT_FOUND,
                    message.as_str(),
                    id,
                    session_id,
                    now,
                )
            }
            Err(ResourceTemplateReadError::Internal(error)) => {
                self.errors
                    .report(McpMiddlewareError::ResourceRead {
                        session_id,
                        uri,
                        error: error.as_str(),
                    })
                    .await;

                send_jsonrpc_error_as_stream(
                    super::mcp_output_contract::JSONRPC_INTERNAL_ERROR,
                    error.as_str(),
                    id,
                    session_id,
                    now,
                )
            }
        }
    }

    /// The body never arrived, so no MCP method ran. `HttpFailResult`
    /// carries its reason as the response content it would have sent.
    async fn report_body_read_failure(&self, session_id: Option<&str>, err: &HttpFailResult) {
        let error = match &err.output {
            HttpOutput::Content {
                status_code,
                content,
                ..
            } => format!("[{}] {}", status_code, String::from_utf8_lossy(content)),
            other => format!("{:?}", other),
        };

        self.errors
            .report(McpMiddlewareError::RequestBodyRead {
                session_id,
                error: error.as_str(),
            })
            .await;
    }

    async fn handle_post_request(
        &self,
        session_id: Option<&str>,
        body: &[u8],
        mut ctx: Option<&mut HttpContext>,
    ) -> Result<HttpOkResult, HttpFailResult> {
        let now = DateTimeAsMicroseconds::now();

        let payload = match super::McpInputPayload::parse(body) {
            Ok(payload) => payload,
            Err(err) => {
                self.errors
                    .report(McpMiddlewareError::PayloadDeserialization {
                        session_id,
                        method: err.method.as_deref().unwrap_or_default(),
                        payload: err.payload.as_str(),
                        error: err.error.as_str(),
                    })
                    .await;

                // Malformed JSON-RPC → HTTP 400 with a standard Parse
                // error body (no SSE framing on plain HTTP errors).
                let body = super::mcp_output_contract::compile_jsonrpc_error_body(
                    super::mcp_output_contract::JSONRPC_PARSE_ERROR,
                    format!("Parse error: {}", err.message).as_str(),
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
            self.errors
                .report(McpMiddlewareError::SessionRejected {
                    session_id: None,
                    http_method: "POST",
                    error: MISSING_SESSION_HEADER,
                })
                .await;

            return Err(HttpFailResult::as_validation_error(MISSING_SESSION_HEADER));
        };

        if !self
            .sessions
            .check_session_and_update_last_used(session_id, now)
        {
            if !self.lazy_session_creation {
                // Spec: 404 signals the session is gone and the client
                // should start over with a new `initialize`.
                self.errors
                    .report(McpMiddlewareError::SessionRejected {
                        session_id: Some(session_id),
                        http_method: "POST",
                        error: UNKNOWN_SESSION,
                    })
                    .await;

                return Err(HttpFailResult::as_not_found(UNKNOWN_SESSION, false));
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
                    self.errors
                        .report(McpMiddlewareError::SessionRejected {
                            session_id: None,
                            http_method: "GET",
                            error: MISSING_SESSION_HEADER,
                        })
                        .await;

                    return Some(
                        HttpFailResult::as_validation_error(MISSING_SESSION_HEADER).into_err(),
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

                self.errors
                    .report(McpMiddlewareError::SessionRejected {
                        session_id: Some(session_id.as_str()),
                        http_method: "GET",
                        error: UNKNOWN_SESSION,
                    })
                    .await;

                return Some(HttpFailResult::as_not_found(UNKNOWN_SESSION, false).into_err());
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
                            self.report_body_read_failure(session_id.as_deref(), &err)
                                .await;
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
                        self.report_body_read_failure(session_id.as_deref(), &err)
                            .await;
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
                    self.errors
                        .report(McpMiddlewareError::SessionRejected {
                            session_id: None,
                            http_method: "DELETE",
                            error: MISSING_SESSION_HEADER,
                        })
                        .await;

                    return Some(
                        HttpFailResult::as_validation_error(MISSING_SESSION_HEADER).into_err(),
                    );
                };

                let removed = self.sessions.delete_session(session_id.as_str()).await;

                if !removed {
                    self.errors
                        .report(McpMiddlewareError::SessionRejected {
                            session_id: Some(session_id.as_str()),
                            http_method: "DELETE",
                            error: UNKNOWN_SESSION,
                        })
                        .await;

                    return Some(HttpFailResult::as_not_found(UNKNOWN_SESSION, false).into_err());
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

    /// A tool with a *required* field, so a call can fail before the tool
    /// ever runs — which is the whole point of the error hook.
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct SearchInput {
        pattern: String,
    }

    #[async_trait::async_trait]
    impl JsonTypeDescription for SearchInput {
        async fn get_description(
            _has_default: bool,
            _with_enum: Option<Vec<rust_extensions::StrOrString<'static>>>,
            _output: bool,
        ) -> my_ai_agent::my_json::json_writer::JsonObjectWriter {
            my_ai_agent::my_json::json_writer::JsonObjectWriter::new()
                .write("type", "object")
                .write_json_object("properties", |properties| {
                    properties.write_json_object("pattern", |pattern| pattern.write("type", "string"))
                })
                .write_json_array("required", |required| required.write("pattern"))
        }
    }

    struct SearchTool;

    impl ToolDefinition for SearchTool {
        const FUNC_NAME: &'static str = "search";
        const DESCRIPTION: &'static str = "Searches by pattern";
    }

    #[async_trait::async_trait]
    impl McpToolCall<SearchInput, EchoOutput> for SearchTool {
        async fn execute_tool_call(&self, model: SearchInput) -> Result<EchoOutput, String> {
            if model.pattern.is_empty() {
                return Err("pattern must not be empty".to_string());
            }

            Ok(EchoOutput {
                echoed: model.pattern,
            })
        }
    }

    struct FailingPrompt;

    impl PromptDefinition for FailingPrompt {
        const PROMPT_NAME: &'static str = "boom";
        const DESCRIPTION: &'static str = "Always fails";

        fn get_argument_descriptions() -> Vec<crate::PromptArgumentDescription> {
            Vec::new()
        }
    }

    #[async_trait::async_trait]
    impl McpPromptService for FailingPrompt {
        async fn execute_prompt(
            &self,
            _arguments: &std::collections::HashMap<String, String>,
        ) -> Result<crate::PromptExecutionResult, String> {
            Err("prompt is broken".to_string())
        }
    }

    /// Output that `serde_json` refuses: a map keyed by something that
    /// can not be a JSON object key. The tool succeeds, the middleware
    /// then can not answer with it.
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct UnserializableOutput {
        rows: std::collections::HashMap<(i32, i32), String>,
    }

    #[async_trait::async_trait]
    impl JsonTypeDescription for UnserializableOutput {
        async fn get_description(
            _has_default: bool,
            _with_enum: Option<Vec<rust_extensions::StrOrString<'static>>>,
            _output: bool,
        ) -> my_ai_agent::my_json::json_writer::JsonObjectWriter {
            my_ai_agent::my_json::json_writer::JsonObjectWriter::new().write("type", "object")
        }
    }

    struct BadOutputTool;

    impl ToolDefinition for BadOutputTool {
        const FUNC_NAME: &'static str = "bad_output";
        const DESCRIPTION: &'static str = "Returns something that can not be serialized";
    }

    #[async_trait::async_trait]
    impl McpToolCall<EchoInput, UnserializableOutput> for BadOutputTool {
        async fn execute_tool_call(
            &self,
            _model: EchoInput,
        ) -> Result<UnserializableOutput, String> {
            let mut rows = std::collections::HashMap::new();
            rows.insert((1, 2), "one".to_string());
            Ok(UnserializableOutput { rows })
        }
    }

    struct FailingResource;

    impl crate::ResourceDefinition for FailingResource {
        const RESOURCE_URI: &'static str = "test://failing";
        const RESOURCE_NAME: &'static str = "failing";
        const DESCRIPTION: &'static str = "Always fails to read";
        const MIME_TYPE: &'static str = "text/plain";
    }

    #[async_trait::async_trait]
    impl McpResourceService for FailingResource {
        async fn read_resource(&self) -> Result<crate::ResourceReadResult, String> {
            Err("disk is on fire".to_string())
        }
    }

    fn text_resource(uri: &str, text: String) -> crate::ResourceReadResult {
        crate::ResourceReadResult {
            contents: vec![crate::ResourceContent {
                uri: uri.to_string(),
                mime_type: "text/markdown".to_string(),
                text: Some(text),
                blob: None,
            }],
        }
    }

    /// Serves `docs://lib/{topic}`: knows two topics, fails on `broken`
    /// and answers `NotFound` for everything else.
    struct DocsTemplate;

    impl ResourceTemplateDefinition for DocsTemplate {
        const URI_TEMPLATE: &'static str = "docs://lib/{topic}";
        const TEMPLATE_NAME: &'static str = "lib-docs";
        const DESCRIPTION: &'static str = "Library docs by topic";
        const MIME_TYPE: &'static str = "text/markdown";
    }

    #[async_trait::async_trait]
    impl McpResourceTemplateService for DocsTemplate {
        async fn read_resource(
            &self,
            uri: &str,
            variables: &std::collections::HashMap<String, String>,
        ) -> Result<crate::ResourceReadResult, ResourceTemplateReadError> {
            let topic = variables.get("topic").expect("the template has `topic`");

            match topic.as_str() {
                "broken" => Err(ResourceTemplateReadError::Internal(
                    "disk is on fire".to_string(),
                )),
                "events-loop" | "hello world" => {
                    Ok(text_resource(uri, format!("topic: {}", topic)))
                }
                _ => Err(ResourceTemplateReadError::NotFound(format!(
                    "Unknown topic `{}`. Available: events-loop",
                    topic
                ))),
            }
        }
    }

    /// Has more literal text than `DocsTemplate`, so `docs://lib/<x>.md`
    /// belongs to it although both templates match.
    struct MarkdownDocsTemplate;

    impl ResourceTemplateDefinition for MarkdownDocsTemplate {
        const URI_TEMPLATE: &'static str = "docs://lib/{topic}.md";
        const TEMPLATE_NAME: &'static str = "lib-docs-md";
        const DESCRIPTION: &'static str = "Library docs by topic, as a file";
        const MIME_TYPE: &'static str = "text/markdown";
    }

    #[async_trait::async_trait]
    impl McpResourceTemplateService for MarkdownDocsTemplate {
        async fn read_resource(
            &self,
            uri: &str,
            variables: &std::collections::HashMap<String, String>,
        ) -> Result<crate::ResourceReadResult, ResourceTemplateReadError> {
            Ok(text_resource(
                uri,
                format!("markdown: {}", variables["topic"]),
            ))
        }
    }

    /// A static resource whose URI fits `DocsTemplate` as well.
    struct PinnedDoc;

    impl ResourceDefinition for PinnedDoc {
        const RESOURCE_URI: &'static str = "docs://lib/pinned";
        const RESOURCE_NAME: &'static str = "pinned";
        const DESCRIPTION: &'static str = "A doc with its own static resource";
        const MIME_TYPE: &'static str = "text/markdown";
    }

    #[async_trait::async_trait]
    impl McpResourceService for PinnedDoc {
        async fn read_resource(&self) -> Result<crate::ResourceReadResult, String> {
            Ok(text_resource(Self::RESOURCE_URI, "pinned".to_string()))
        }
    }

    fn middleware_with_echo_tool() -> McpMiddleware {
        let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
        mcp.register_tool_call(Arc::new(EchoTool));
        mcp.register_tool_call(Arc::new(SearchTool));
        mcp.register_tool_call(Arc::new(BadOutputTool));
        mcp.register_prompt(Arc::new(FailingPrompt));
        mcp.register_resource(Arc::new(FailingResource));
        mcp
    }

    /// Owned copy of what the error hook was handed — the event itself is
    /// borrowed and gone by the time a test looks at it.
    #[derive(Debug, Clone, PartialEq)]
    enum Recorded {
        ToolInputDeserialization {
            session_id: String,
            tool_name: String,
            arguments: String,
            error: String,
        },
        ToolExecution {
            session_id: String,
            tool_name: String,
            arguments: String,
            error: String,
        },
        ToolNotFound {
            session_id: String,
            tool_name: String,
            arguments: String,
        },
        PromptExecution {
            session_id: String,
            prompt_name: String,
            error: String,
        },
        PayloadDeserialization {
            session_id: Option<String>,
            method: String,
            payload: String,
        },
        ToolOutputSerialization {
            session_id: String,
            tool_name: String,
        },
        PromptNotFound {
            session_id: String,
            prompt_name: String,
        },
        ResourceNotFound {
            session_id: String,
            method: String,
            uri: String,
        },
        ResourceRead {
            session_id: String,
            uri: String,
            error: String,
        },
        MethodNotFound {
            session_id: String,
            method: String,
            payload: String,
        },
        SessionRejected {
            session_id: Option<String>,
            http_method: String,
            error: String,
        },
        /// The variants a test only needs to see the shape of are folded
        /// into their `Display` line plus the session they came from.
        Other {
            session_id: Option<String>,
            line: String,
        },
    }

    #[derive(Default)]
    struct RecordingErrorHook {
        errors: parking_lot::Mutex<Vec<Recorded>>,
    }

    impl RecordingErrorHook {
        fn errors(&self) -> Vec<Recorded> {
            self.errors.lock().clone()
        }
    }

    #[async_trait::async_trait]
    impl McpMiddlewareErrorHook for RecordingErrorHook {
        async fn on_error(&self, err: McpMiddlewareError<'_>) {
            let recorded = match err {
                McpMiddlewareError::ToolInputDeserialization {
                    session_id,
                    tool_name,
                    arguments,
                    error,
                } => Recorded::ToolInputDeserialization {
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                    arguments: arguments.to_string(),
                    error: error.to_string(),
                },
                McpMiddlewareError::ToolExecution {
                    session_id,
                    tool_name,
                    arguments,
                    error,
                } => Recorded::ToolExecution {
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                    arguments: arguments.to_string(),
                    error: error.to_string(),
                },
                McpMiddlewareError::ToolNotFound {
                    session_id,
                    tool_name,
                    arguments,
                } => Recorded::ToolNotFound {
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                    arguments: arguments.to_string(),
                },
                McpMiddlewareError::PromptExecution {
                    session_id,
                    prompt_name,
                    error,
                    ..
                } => Recorded::PromptExecution {
                    session_id: session_id.to_string(),
                    prompt_name: prompt_name.to_string(),
                    error: error.to_string(),
                },
                McpMiddlewareError::PayloadDeserialization {
                    session_id,
                    method,
                    payload,
                    ..
                } => Recorded::PayloadDeserialization {
                    session_id: session_id.map(|id| id.to_string()),
                    method: method.to_string(),
                    payload: payload.to_string(),
                },
                McpMiddlewareError::ToolOutputSerialization {
                    session_id,
                    tool_name,
                    ..
                } => Recorded::ToolOutputSerialization {
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                },
                McpMiddlewareError::PromptNotFound {
                    session_id,
                    prompt_name,
                    ..
                } => Recorded::PromptNotFound {
                    session_id: session_id.to_string(),
                    prompt_name: prompt_name.to_string(),
                },
                McpMiddlewareError::ResourceNotFound {
                    session_id,
                    method,
                    uri,
                } => Recorded::ResourceNotFound {
                    session_id: session_id.to_string(),
                    method: method.to_string(),
                    uri: uri.to_string(),
                },
                McpMiddlewareError::ResourceRead {
                    session_id,
                    uri,
                    error,
                } => Recorded::ResourceRead {
                    session_id: session_id.to_string(),
                    uri: uri.to_string(),
                    error: error.to_string(),
                },
                McpMiddlewareError::MethodNotFound {
                    session_id,
                    method,
                    payload,
                } => Recorded::MethodNotFound {
                    session_id: session_id.to_string(),
                    method: method.to_string(),
                    payload: payload.to_string(),
                },
                McpMiddlewareError::SessionRejected {
                    session_id,
                    http_method,
                    error,
                } => Recorded::SessionRejected {
                    session_id: session_id.map(|id| id.to_string()),
                    http_method: http_method.to_string(),
                    error: error.to_string(),
                },
                other => Recorded::Other {
                    session_id: other.session_id().map(|id| id.to_string()),
                    line: other.to_string(),
                },
            };

            self.errors.lock().push(recorded);
        }
    }

    fn middleware_with_error_hook() -> (McpMiddleware, Arc<RecordingErrorHook>) {
        let hook = Arc::new(RecordingErrorHook::default());
        let mut mcp = middleware_with_echo_tool();
        mcp.register_error_hook(hook.clone());
        (mcp, hook)
    }

    fn middleware_with_templates() -> (McpMiddleware, Arc<RecordingErrorHook>) {
        let (mut mcp, hook) = middleware_with_error_hook();
        mcp.register_resource_template(Arc::new(DocsTemplate));
        mcp.register_resource_template(Arc::new(MarkdownDocsTemplate));
        mcp.register_resource(Arc::new(PinnedDoc));
        (mcp, hook)
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

    /// The JSON-RPC message of a one-frame SSE response.
    fn sse_json(body: &str) -> serde_json::Value {
        let json = body
            .strip_prefix("data: ")
            .expect("an SSE data frame")
            .trim_end();
        serde_json::from_str(json).expect("valid json")
    }

    async fn post(mcp: &McpMiddleware, session_id: &str, body: String) -> serde_json::Value {
        let result = mcp
            .handle_post_request(Some(session_id), body.as_bytes(), None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        sse_json(&body)
    }

    async fn read_resource(mcp: &McpMiddleware, session_id: &str, uri: &str) -> serde_json::Value {
        let body = format!(
            r#"{{"jsonrpc":"2.0","method":"resources/read","id":5,"params":{{"uri":"{}"}}}}"#,
            uri
        );
        post(mcp, session_id, body).await
    }

    #[tokio::test]
    async fn resource_templates_list_returns_the_registered_templates() {
        let (mcp, _) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let body = r#"{"jsonrpc":"2.0","method":"resources/templates/list","id":8}"#;
        let parsed = post(&mcp, session_id.as_str(), body.to_string()).await;

        let templates = parsed["result"]["resourceTemplates"].as_array().unwrap();
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0]["uriTemplate"], "docs://lib/{topic}");
        assert_eq!(templates[0]["name"], "lib-docs");
        assert_eq!(templates[0]["description"], "Library docs by topic");
        assert_eq!(templates[0]["mimeType"], "text/markdown");
        assert_eq!(templates[1]["uriTemplate"], "docs://lib/{topic}.md");
    }

    #[tokio::test]
    async fn templates_are_not_listed_as_resources() {
        let (mcp, _) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let body = r#"{"jsonrpc":"2.0","method":"resources/list","id":8}"#;
        let parsed = post(&mcp, session_id.as_str(), body.to_string()).await;

        let uris: Vec<&str> = parsed["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|resource| resource["uri"].as_str().unwrap())
            .collect();
        assert_eq!(uris, vec!["docs://lib/pinned", "test://failing"]);
    }

    #[tokio::test]
    async fn reading_a_templated_uri_hands_the_variables_to_the_handler() {
        let (mcp, hook) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/events-loop").await;

        let content = &parsed["result"]["contents"][0];
        assert_eq!(content["uri"], "docs://lib/events-loop");
        assert_eq!(content["text"], "topic: events-loop");
        assert!(hook.errors().is_empty());
    }

    #[tokio::test]
    async fn a_percent_encoded_variable_reaches_the_handler_decoded() {
        let (mcp, _) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/hello%20world").await;

        let content = &parsed["result"]["contents"][0];
        assert_eq!(content["uri"], "docs://lib/hello%20world");
        assert_eq!(content["text"], "topic: hello world");
    }

    #[tokio::test]
    async fn the_template_with_more_literal_text_wins() {
        let (mcp, _) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/intro.md").await;

        assert_eq!(parsed["result"]["contents"][0]["text"], "markdown: intro");
    }

    #[tokio::test]
    async fn an_exact_static_uri_wins_over_a_template() {
        let (mcp, _) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/pinned").await;

        assert_eq!(parsed["result"]["contents"][0]["text"], "pinned");
    }

    #[tokio::test]
    async fn a_uri_no_template_matches_is_resource_not_found() {
        let (mcp, hook) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        for uri in [
            "docs://lib/a/b",
            "docs://lib/a%2Fb",
            "docs://lib/..",
            "docs://other/events-loop",
        ] {
            let parsed = read_resource(&mcp, session_id.as_str(), uri).await;

            assert_eq!(parsed["error"]["code"], -32002, "{}", uri);
            assert_eq!(
                parsed["error"]["message"],
                format!("Resource not found: {}", uri)
            );
            assert_eq!(
                hook.errors().pop().unwrap(),
                Recorded::ResourceNotFound {
                    session_id: session_id.clone(),
                    method: "resources/read".to_string(),
                    uri: uri.to_string(),
                }
            );
        }
    }

    #[tokio::test]
    async fn not_found_from_the_handler_is_resource_not_found_with_its_message() {
        let (mcp, hook) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/nope").await;

        assert_eq!(parsed["error"]["code"], -32002);
        assert_eq!(
            parsed["error"]["message"],
            "Unknown topic `nope`. Available: events-loop"
        );
        assert_eq!(
            hook.errors(),
            vec![Recorded::ResourceNotFound {
                session_id,
                method: "resources/read".to_string(),
                uri: "docs://lib/nope".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn an_internal_error_from_the_handler_is_an_internal_error() {
        let (mcp, hook) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let parsed = read_resource(&mcp, session_id.as_str(), "docs://lib/broken").await;

        assert_eq!(parsed["error"]["code"], -32603);
        assert_eq!(parsed["error"]["message"], "disk is on fire");
        assert_eq!(
            hook.errors(),
            vec![Recorded::ResourceRead {
                session_id,
                uri: "docs://lib/broken".to_string(),
                error: "disk is on fire".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn subscribe_accepts_a_uri_a_template_matches() {
        let (mcp, hook) = middleware_with_templates();
        let session_id = initialize_session(&mcp).await;

        let subscribe = |uri: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"resources/subscribe","id":9,"params":{{"uri":"{}"}}}}"#,
                uri
            )
        };

        let parsed = post(&mcp, session_id.as_str(), subscribe("docs://lib/anything")).await;
        assert!(parsed.get("error").is_none());
        assert!(parsed["result"].is_object());
        assert!(hook.errors().is_empty());

        let parsed = post(&mcp, session_id.as_str(), subscribe("docs://lib/a/b")).await;
        assert_eq!(parsed["error"]["code"], -32002);
    }

    #[test]
    #[should_panic(expected = "Resource template `docs://{+path}` is not supported")]
    fn registering_a_template_with_an_operator_panics() {
        struct PathTemplate;

        impl ResourceTemplateDefinition for PathTemplate {
            const URI_TEMPLATE: &'static str = "docs://{+path}";
            const TEMPLATE_NAME: &'static str = "path";
            const DESCRIPTION: &'static str = "Reserved expansion is level 2";
            const MIME_TYPE: &'static str = "text/plain";
        }

        #[async_trait::async_trait]
        impl McpResourceTemplateService for PathTemplate {
            async fn read_resource(
                &self,
                _uri: &str,
                _variables: &std::collections::HashMap<String, String>,
            ) -> Result<crate::ResourceReadResult, ResourceTemplateReadError> {
                unreachable!("never registered")
            }
        }

        let mut mcp = McpMiddleware::new("/mcp", "test-server", "0.0.1", "test instructions");
        mcp.register_resource_template(Arc::new(PathTemplate));
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

    /// Arguments that do not match the tool's `inputSchema` fail twice on
    /// the way out — the executor refuses them, and the middleware sees
    /// the refusal as a failed call — and both lines went to the console
    /// before hooks existed. The hook gets both, in that order.
    #[tokio::test]
    async fn bad_tool_arguments_are_reported_with_the_session_and_the_arguments() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"search","arguments":{"project":"mt-risks","query":"Account groups"}}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (status, _, _) = read_sse_response(result).await;
        assert_eq!(status, 200);

        let errors = hook.errors();
        assert_eq!(errors.len(), 2, "got {:?}", errors);

        match &errors[0] {
            Recorded::ToolInputDeserialization {
                session_id: reported_session,
                tool_name,
                arguments,
                error,
            } => {
                assert_eq!(reported_session, &session_id);
                assert_eq!(tool_name, "search");
                assert_eq!(
                    arguments,
                    r#"{"project":"mt-risks","query":"Account groups"}"#
                );
                assert!(error.contains("missing field `pattern`"), "{}", error);
            }
            other => panic!("expected ToolInputDeserialization, got {:?}", other),
        }

        match &errors[1] {
            Recorded::ToolExecution {
                session_id: reported_session,
                tool_name,
                error,
                ..
            } => {
                assert_eq!(reported_session, &session_id);
                assert_eq!(tool_name, "search");
                assert!(error.contains("Can not deserialize input data"), "{}", error);
            }
            other => panic!("expected ToolExecution, got {:?}", other),
        }
    }

    /// The refusal the client gets carries the tool's own schema, so a
    /// model that invented a field name can fix it on the next turn
    /// instead of guessing.
    #[tokio::test]
    async fn deserialization_refusal_carries_the_expected_schema() {
        let mcp = middleware_with_echo_tool();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"search","arguments":{"query":"Account groups"}}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;

        assert!(body.contains(r#""isError":true"#));
        assert!(body.contains("missing field"), "{}", body);
        assert!(body.contains("Expected schema"), "{}", body);
        // ...and the schema is the real one, naming the field the model
        // should have sent.
        assert!(body.contains(r#"required"#), "{}", body);
        assert!(body.contains(r#"pattern"#), "{}", body);
    }

    /// A tool that ran and returned `Err` is a different event from one
    /// whose arguments never parsed.
    #[tokio::test]
    async fn a_failing_tool_is_reported_once_as_tool_execution() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"search","arguments":{"pattern":""}}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        assert!(body.contains("pattern must not be empty"));

        assert_eq!(
            hook.errors(),
            vec![Recorded::ToolExecution {
                session_id,
                tool_name: "search".to_string(),
                arguments: r#"{"pattern":""}"#.to_string(),
                error: "pattern must not be empty".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"nope","arguments":{"a":1}}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        // The wire answer is untouched by the hook.
        assert!(body.contains("Unknown tool: nope"));

        assert_eq!(
            hook.errors(),
            vec![Recorded::ToolNotFound {
                session_id,
                tool_name: "nope".to_string(),
                arguments: r#"{"a":1}"#.to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn a_failing_prompt_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"prompts/get","id":5,"params":{"name":"boom"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        assert!(body.contains("prompt is broken"));

        assert_eq!(
            hook.errors(),
            vec![Recorded::PromptExecution {
                session_id,
                prompt_name: "boom".to_string(),
                error: "prompt is broken".to_string(),
            }]
        );
    }

    /// `params` that do not match the method's own contract: the method
    /// is known, so it is named, and the payload is the `params` object.
    #[tokio::test]
    async fn unparsable_params_are_reported_with_the_method() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"tool":"search"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let ok = result.expect("400 is returned as ok-result with JSON body");
        assert_eq!(ok.output.get_status_code(), 400);

        assert_eq!(
            hook.errors(),
            vec![Recorded::PayloadDeserialization {
                session_id: Some(session_id),
                method: "tools/call".to_string(),
                payload: r#"{"tool":"search"}"#.to_string(),
            }]
        );
    }

    /// A body that is not even JSON-RPC: no method to name, and the
    /// payload is the whole body.
    #[tokio::test]
    async fn an_unusable_envelope_is_reported_without_a_method() {
        let (mcp, hook) = middleware_with_error_hook();

        let result = mcp
            .handle_post_request(None, b"this is not json", None)
            .await;
        let ok = result.expect("400 is returned as ok-result with JSON body");
        assert_eq!(ok.output.get_status_code(), 400);

        assert_eq!(
            hook.errors(),
            vec![Recorded::PayloadDeserialization {
                session_id: None,
                method: String::new(),
                payload: "this is not json".to_string(),
            }]
        );
    }

    /// The tool ran fine but its own output type can not be serialized.
    /// This used to be an `unwrap()`: the response task died and the
    /// client was left with an empty stream and no clue why.
    #[tokio::test]
    async fn a_tool_result_that_can_not_be_serialized_is_reported_not_panicked() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"bad_output"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (status, body, _) = read_sse_response(result).await;

        // The client is told, in-band, instead of getting nothing at all.
        assert_eq!(status, 200);
        assert!(body.contains(r#""isError":true"#), "{}", body);
        assert!(body.contains("Can not serialize the result of bad_output"), "{}", body);

        let errors = hook.errors();
        assert_eq!(errors.len(), 2, "got {:?}", errors);
        assert_eq!(
            errors[0],
            Recorded::ToolOutputSerialization {
                session_id,
                tool_name: "bad_output".to_string(),
            }
        );
        assert!(matches!(errors[1], Recorded::ToolExecution { .. }));
    }

    #[tokio::test]
    async fn unknown_prompt_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"prompts/get","id":5,"params":{"name":"nope"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        assert!(body.contains("Unknown prompt: nope"));

        assert_eq!(
            hook.errors(),
            vec![Recorded::PromptNotFound {
                session_id,
                prompt_name: "nope".to_string(),
            }]
        );
    }

    /// Both `resources/read` and `resources/subscribe` report the URI,
    /// and say which of the two asked for it.
    #[tokio::test]
    async fn unknown_resource_is_reported_for_read_and_subscribe() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        for (body, method) in [
            (
                br#"{"jsonrpc":"2.0","method":"resources/read","id":5,"params":{"uri":"res://missing"}}"#.as_slice(),
                "resources/read",
            ),
            (
                br#"{"jsonrpc":"2.0","method":"resources/subscribe","id":6,"params":{"uri":"res://missing"}}"#.as_slice(),
                "resources/subscribe",
            ),
        ] {
            let result = mcp
                .handle_post_request(Some(session_id.as_str()), body, None)
                .await;
            let (_, body, _) = read_sse_response(result).await;
            assert!(body.contains(r#""code":-32002"#));

            assert_eq!(
                hook.errors().pop().unwrap(),
                Recorded::ResourceNotFound {
                    session_id: session_id.clone(),
                    method: method.to_string(),
                    uri: "res://missing".to_string(),
                }
            );
        }
    }

    #[tokio::test]
    async fn a_failing_resource_read_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"resources/read","id":5,"params":{"uri":"test://failing"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        assert!(body.contains("disk is on fire"));

        assert_eq!(
            hook.errors(),
            vec![Recorded::ResourceRead {
                session_id,
                uri: "test://failing".to_string(),
                error: "disk is on fire".to_string(),
            }]
        );
    }

    /// An unimplemented method is reported whether it was a request or a
    /// notification — a silently swallowed notification is exactly what a
    /// host wants to find out about.
    #[tokio::test]
    async fn an_unsupported_method_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();
        let session_id = initialize_session(&mcp).await;

        let body = br#"{"jsonrpc":"2.0","method":"logging/setLevel","id":7,"params":{"level":"debug"}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (_, body, _) = read_sse_response(result).await;
        assert!(body.contains(r#""code":-32601"#));

        assert_eq!(
            hook.errors(),
            vec![Recorded::MethodNotFound {
                session_id,
                method: "logging/setLevel".to_string(),
                payload: r#"{"level":"debug"}"#.to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn a_post_without_the_session_header_is_reported() {
        let (mcp, hook) = middleware_with_error_hook();

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp.handle_post_request(None, body, None).await;
        assert_eq!(result.err().unwrap().output.get_status_code(), 400);

        assert_eq!(
            hook.errors(),
            vec![Recorded::SessionRejected {
                session_id: None,
                http_method: "POST".to_string(),
                error: "Missing mcp-session-id header".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn a_post_on_an_unknown_session_is_reported_when_lazy_creation_is_off() {
        let hook = Arc::new(RecordingErrorHook::default());
        let mut mcp = middleware_with_echo_tool().disabled_lazy_session_creation();
        mcp.register_error_hook(hook.clone());

        let body = br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let result = mcp
            .handle_post_request(Some("no-such-session"), body, None)
            .await;
        assert_eq!(result.err().unwrap().output.get_status_code(), 404);

        assert_eq!(
            hook.errors(),
            vec![Recorded::SessionRejected {
                session_id: Some("no-such-session".to_string()),
                http_method: "POST".to_string(),
                error: "Unknown MCP session".to_string(),
            }]
        );
    }

    /// The hook observes, it does not decide: the client gets the same
    /// refusal with or without one registered.
    #[tokio::test]
    async fn the_hook_does_not_change_what_the_client_is_told() {
        let call = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"search","arguments":{"query":"x"}}}"#;

        let without_hook = {
            let mcp = middleware_with_echo_tool();
            let session_id = initialize_session(&mcp).await;
            let result = mcp
                .handle_post_request(Some(session_id.as_str()), call, None)
                .await;
            read_sse_response(result).await.1
        };

        let with_hook = {
            let (mcp, hook) = middleware_with_error_hook();
            let session_id = initialize_session(&mcp).await;
            let result = mcp
                .handle_post_request(Some(session_id.as_str()), call, None)
                .await;
            let body = read_sse_response(result).await.1;
            assert_eq!(hook.errors().len(), 2);
            body
        };

        assert_eq!(without_hook, with_hook);
    }

    /// Requirement: a host hook that panics must not take the request
    /// down, and the response must be the usual one.
    #[tokio::test]
    async fn a_panicking_hook_does_not_break_the_request() {
        struct Panicking;

        #[async_trait::async_trait]
        impl McpMiddlewareErrorHook for Panicking {
            async fn on_error(&self, _err: McpMiddlewareError<'_>) {
                panic!("host hook is broken");
            }
        }

        let mut mcp = middleware_with_echo_tool();
        mcp.register_error_hook(Arc::new(Panicking));
        let session_id = initialize_session(&mcp).await;

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let body = br#"{"jsonrpc":"2.0","method":"tools/call","id":5,"params":{"name":"search","arguments":{"query":"x"}}}"#;
        let result = mcp
            .handle_post_request(Some(session_id.as_str()), body, None)
            .await;
        let (status, body, _) = read_sse_response(result).await;

        std::panic::set_hook(previous);

        assert_eq!(status, 200);
        assert!(body.contains(r#""isError":true"#));
        assert!(body.contains("missing field"));
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
