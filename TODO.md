# TODO — путь к полноценному MCP-серверу

Список того, что ещё нужно закрыть в `mcp-server-middleware`, чтобы downstream-сервис, подключив только этот крейт, был полноценным MCP-сервером по спеке `2025-11-25`.

## Протокол

- [ ] **`resources/subscribe` → реальные обновления.** Сейчас [mcp_middleware.rs](src/mcp_middleware/mcp_middleware.rs) на `SubscribeResource` просто возвращает первую версию ресурса и забывает. Нужно: хранить per-session список подписанных URI, добавить вариант `ResourceUpdated { uri }` в `McpSocketUpdateEvent`, и публичный метод `McpMiddleware::notify_resource_updated(uri)` — фан-аут только на тех, кто подписан на этот URI.
- [ ] **`resources/unsubscribe`** — метод спеки, сейчас не парсится в [mcp_payload.rs](src/mcp_middleware/mcp_payload.rs).
- [ ] **`logging/setLevel` + `notifications/message`** — сервер должен уметь принимать желаемый log level и слать клиенту структурированные логи. Нужны: парсинг метода, поле `log_level` в `McpSession`, вариант `McpSocketUpdateEvent::LogMessage { level, logger, data }`, публичный API `McpMiddleware::log(level, logger, data)`.
- [ ] **`notifications/progress`** — для долгих tool-вызовов клиент посылает `progressToken` в `params._meta.progressToken`. Сейчас он игнорируется. Нужно: пробросить токен в `McpToolCall::execute_tool_call` (через контекст-аргумент), завести `ProgressReporter`, который шлёт `notifications/progress` в SSE сессии-инициатора.
- [ ] **`completion/complete`** — auto-completion для аргументов prompts/resources. Не реализовано вообще.
- [ ] **`roots/list` + `notifications/roots/list_changed`** — клиент-side concept, но сервер должен уметь спросить. Опционально, если хотим инициировать с сервера.
- [ ] **`elicitation/create`** — server→client запрос на ввод данных. Опционально, новая фича спеки.
- [ ] **Sampling (`sampling/createMessage`)** — server→client LLM-запрос. Опционально, для агентских сценариев.
- [ ] **`_meta` / `cursor` / `progressToken`** — сейчас вырезаются на парсинге. Должны проходить сквозняком и быть доступны хендлерам.
- [ ] **JSON-RPC batch requests** — `try_parse` ждёт один объект, не массив. Спека требует поддержку батчей.
- [ ] **JSON-RPC error codes** — сейчас всё мапится в `as_fatal_error` (HTTP 500). Должны возвращаться JSON-RPC ошибки `-32700`/`-32600`/`-32601`/`-32602`/`-32603` с правильной структурой.
- [ ] **`ping` от сервера к клиенту** — сейчас умеем только отвечать; для liveness-чека своих сессий нужен `McpSocketUpdateEvent::Ping`.

## Tools / Prompts / Resources API

- [ ] **`tools/list_changed` авто** — сейчас фан-аут только если потребитель явно зовёт `notify_tools_changed()`. Опционально: триггерить из `register_tool_call` после `initialize` (если регистрация рантайм-динамическая).
- [ ] **Resource templates (`resources/templates/list`)** — параметризованные URI типа `file:///{path}`. Не реализовано, типов нет.
- [ ] **Tool annotations** — `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`. Должны попадать в `tools/list`. Сейчас `ToolDefinition` их не выставляет.
- [ ] **Tool `_meta` и `title`** — отдельный человекочитаемый title, помимо `name`/`description`.
- [ ] **Prompts с image/audio/embedded resource контентом** — сейчас `PromptExecutionResult.message: String`. Спека разрешает массив content-блоков разных типов.
- [ ] **Multi-message prompts** — сейчас всегда один user-message. Должна быть последовательность ролей.
- [ ] **`structuredContent` валидация против `outputSchema`** — сейчас `compile_execute_tool_call_response` пишет результат как есть, без проверки соответствия схеме.

## Сессии и транспорт

- [ ] **TTL и eviction сессий.** `last_access` обновляется, но никто не чистит. Нужен фоновый таск, который удаляет сессии старше N минут.
- [ ] **Backpressure на SSE.** Канал `mpsc::channel(32)`. При медленном клиенте broadcast будет молча терять — `let _ = sender.send(...)`. Решить: drop-oldest, drop-newest, или закрывать сессию.
- [ ] **`Last-Event-ID` / resumability.** Спека позволяет клиенту переподключиться и догнать пропущенные события. Сейчас не поддерживается.
- [ ] **`Mcp-Protocol-Version` header** — клиент в каждом запросе должен отправлять, сервер — валидировать совместимость.
- [ ] **CORS / Origin валидация** — для браузерных клиентов, по спеке транспорта обязательно.
- [ ] **Authorization (OAuth 2.1)** — спека MCP HTTP transport ссылается на отдельную auth-спеку. Сейчас единственный «auth» — наличие `mcp-session-id`.

## Качество и инфраструктура

- [ ] **Логирование через `tracing` вместо `println!`** по всему [mcp_middleware.rs](src/mcp_middleware/mcp_middleware.rs).
- [ ] **Тесты.** Сейчас один `test_init_payload`. Нужны:
  - парсинг всех методов (включая невалидный JSON);
  - `tools/call` end-to-end через тестовый handler;
  - сессии: создание, валидация, DELETE, expire;
  - пагинация resources/list на >100 ресурсах;
  - SSE: подписка, broadcast, clear_sender при разорванном клиенте.
- [ ] **Doc-comments на публичный API** — `McpMiddleware`, traits, `notify_*`. Сейчас почти всё без `///`.
- [ ] **README sync** — обновить под текущий API (re-exports, `notify_*`, DELETE).
- [ ] **CHANGELOG + версионирование** — `Cargo.toml` всё ещё `0.1.0`.
- [ ] **`McpSessions` и `McpResources` — `Default` impl** (clippy просит).
- [ ] **Прочистка clippy** — 40 warnings (needless_return, needless_borrow и т.п.).

## Опционально (nice-to-have)

- [ ] **Telemetry-хуки** — счётчики вызовов tools, латенси, размер payload.
- [ ] **Schema caching** — `get_input_params/get_output_params` зовут `JsonTypeDescription::get_description` каждый раз. Можно кешировать после первого `tools/list`.
- [ ] **Configurable `PAGE_SIZE`** для resources pagination — сейчас константа `100`.
- [ ] **WebSocket transport** — спека упоминает как альтернативу HTTP+SSE.
