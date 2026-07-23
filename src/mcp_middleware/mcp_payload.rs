use std::collections::HashMap;

use my_ai_agent::my_json::json_reader::{JsonFirstLineIterator, JsonValueRef};
use my_ai_agent::my_json::json_writer::JsonValueWriter;
use serde::{Deserialize, Serialize};

/// JSON-RPC request id. Per the JSON-RPC 2.0 spec an id is a string, a
/// number or null, and the response MUST echo it back exactly as
/// received — hence the dedicated [`RequestId::Raw`] variant which
/// preserves non-i64 numeric tokens (e.g. `1.5`) byte-identically.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestId {
    Int(i64),
    Str(String),
    /// Numeric token that does not fit i64 (float, big int). Written
    /// back verbatim, without quotes.
    Raw(String),
    Null,
}

impl RequestId {
    pub fn parse(value: &JsonValueRef) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::Null);
        }

        if value.is_string() {
            let Some(value) = value.as_str() else {
                return Err("Can not read request id as a string".to_string());
            };
            return Ok(Self::Str(value.to_string()));
        }

        let Some(raw) = value.as_raw_str() else {
            return Err("Can not read request id".to_string());
        };

        if let Ok(int_value) = raw.parse::<i64>() {
            return Ok(Self::Int(int_value));
        }

        Ok(Self::Raw(raw.to_string()))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }
}

impl JsonValueWriter for &RequestId {
    const IS_ARRAY: bool = false;
    fn write(&self, dest: &mut String) {
        match self {
            RequestId::Int(value) => dest.push_str(value.to_string().as_str()),
            // Delegate to the &str writer so the value gets escaped.
            RequestId::Str(value) => {
                let as_str: &str = value.as_str();
                JsonValueWriter::write(&as_str, dest);
            }
            RequestId::Raw(value) => dest.push_str(value.as_str()),
            RequestId::Null => dest.push_str("null"),
        }
    }
}
#[derive(Debug)]
pub enum McpInputData {
    Initialize(InitializeMpcContract),
    ResourcesList(ResourcesListModel),
    ResourceTemplatesList,
    ReadResource(ReadResourceModel),
    SubscribeResource(SubscribeResourceModel),
    UnsubscribeResource(UnsubscribeResourceModel),
    NotificationsInitialize,
    /// Any other `notifications/*` method. Per the Streamable HTTP
    /// transport notifications are accepted with `202` and ignored if
    /// the server has no handler for them.
    Notification { method: String },
    ToolsList,
    PromptsList,
    ExecuteToolCall(ExecuteToolCallModel),
    GetPrompt(GetPromptModel),
    Ping,
    /// Server-originated request response sent back by the client
    /// (used for `elicitation/create` responses). Either `result_json`
    /// or `error_json` is populated, never both.
    ServerResponse {
        result_json: Option<String>,
        error_json: Option<String>,
    },
    Other { method: String, data: String },
}

impl McpInputData {
    pub fn from_str(method: &str, params: String) -> Result<Self, String> {
        match method {
            "initialize" => {
                let params = serde_json::from_str(&params).map_err(|err| {
                    format!("Can not deserialize initialize data: {}. Err: {:?}", params, err)
                })?;
                Ok(Self::Initialize(params))
            }
            "notifications/initialized" => Ok(Self::NotificationsInitialize),
            "resources/templates/list" => Ok(Self::ResourceTemplatesList),
            "resources/unsubscribe" => {
                let model: UnsubscribeResourceModel =
                    serde_json::from_str(&params).map_err(|err| {
                        format!(
                            "Can not deserialize unsubscribe resource data: {}. Err: {:?}",
                            params, err
                        )
                    })?;
                Ok(Self::UnsubscribeResource(model))
            }
            "resources/list" => {
                let model: Result<ResourcesListModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => Ok(Self::ResourcesList(model)),
                    Err(_) => {
                        // If params is empty or invalid, use default (no cursor)
                        Ok(Self::ResourcesList(ResourcesListModel { cursor: None }))
                    }
                }
            }
            "resources/read" => {
                let model: ReadResourceModel = serde_json::from_str(&params).map_err(|err| {
                    format!(
                        "Can not deserialize read resource data: {}. Err: {:?}",
                        params, err
                    )
                })?;
                Ok(Self::ReadResource(model))
            }
            "resources/subscribe" => {
                let model: SubscribeResourceModel =
                    serde_json::from_str(&params).map_err(|err| {
                        format!(
                            "Can not deserialize subscribe resource data: {}. Err: {:?}",
                            params, err
                        )
                    })?;
                Ok(Self::SubscribeResource(model))
            }
            "tools/list" => Ok(Self::ToolsList),
            "prompts/list" => Ok(Self::PromptsList),
            "prompts/get" => {
                let model: GetPromptModel = serde_json::from_str(&params).map_err(|err| {
                    format!(
                        "Can not deserialize get prompt data: {}. Err: {:?}",
                        params, err
                    )
                })?;
                Ok(Self::GetPrompt(model))
            }
            "ping" => Ok(Self::Ping),
            "tools/call" => {
                let model: ExecuteToolCallModel =
                    serde_json::from_str(&params).map_err(|err| {
                        format!(
                            "Can not deserialize tool call data: {}. Err: {:?}",
                            params, err
                        )
                    })?;
                Ok(Self::ExecuteToolCall(model))
            }
            method if method.starts_with("notifications/") => Ok(Self::Notification {
                method: method.to_string(),
            }),
            _ => Ok(Self::Other {
                method: method.to_string(),
                data: params.to_string(),
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteToolCallModel {
    pub name: String,
    /// Optional per spec — tools with no input are called without it.
    #[serde(default = "default_tool_call_arguments")]
    pub arguments: serde_json::Value,
}

fn default_tool_call_arguments() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPromptModel {
    pub name: String,
    pub arguments: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResourcesListModel {
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadResourceModel {
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscribeResourceModel {
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnsubscribeResourceModel {
    pub uri: String,
}

#[derive(Debug)]
pub struct McpInputPayload {
    pub _version: String,
    pub id: RequestId,
    pub data: McpInputData,
}

impl McpInputPayload {
    pub fn try_parse(src: &[u8]) -> Result<Self, String> {
        let json_iterator = JsonFirstLineIterator::new(src);

        let mut version: Option<String> = None;
        let mut method = None;
        let mut id = RequestId::Null;
        let mut params = None;
        let mut result_json: Option<String> = None;
        let mut error_json: Option<String> = None;

        while let Some(item) = json_iterator.get_next() {
            let (name, value) = item.map_err(|err| format!("{:?}", err))?;

            let name = name.as_str().map_err(|err| format!("{:?}", err))?;

            match name.as_str() {
                "jsonrpc" => {
                    version = value.as_str().map(|v| v.to_string());
                }
                "method" => {
                    method = value.as_str().map(|v| v.to_short_string());
                }
                "id" => {
                    id = RequestId::parse(&value)?;
                }
                "params" => {
                    params = value.as_str().map(|v| v.to_string());
                }
                "result" => {
                    result_json = value.as_str().map(|v| v.to_string());
                }
                "error" => {
                    error_json = value.as_str().map(|v| v.to_string());
                }
                _ => {}
            }
        }

        let Some(version) = version else {
            return Err("Version is null".to_string());
        };

        // JSON-RPC response (no `method`, has `id` and `result`/`error`) →
        // routed to ServerResponse so the middleware can resolve the
        // matching pending server-initiated request (e.g. elicitation).
        if method.is_none() && !id.is_null() && (result_json.is_some() || error_json.is_some()) {
            return Ok(Self {
                _version: version.to_string(),
                id,
                data: McpInputData::ServerResponse {
                    result_json,
                    error_json,
                },
            });
        }

        let Some(method) = method else {
            return Err("Method is null".to_string());
        };

        let data = match params {
            Some(params) => McpInputData::from_str(method.as_str(), params)?,
            None => McpInputData::from_str(method.as_str(), String::new())?,
        };

        Ok(Self {
            _version: version.to_string(),
            id,
            data,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeMpcContract {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
    /// Who is on the other end. The middleware itself does not use it;
    /// it is kept so a host that re-parses the `initialize` body from
    /// the [`crate::McpConnectionInfo::on_connected`] context can name
    /// the client without writing its own contract.
    #[serde(rename = "clientInfo", default)]
    pub client_info: Option<ClientInfo>,
}

/// `clientInfo` of the `initialize` request. Everything is optional —
/// the spec requires `name` and `version`, but a missing one must not
/// fail the handshake.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ClientInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Subset of client capabilities the server cares about. Unknown fields
/// in the wire payload are silently dropped.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Presence of this field (any JSON value) signals that the client
    /// supports `elicitation/create`. The MCP spec advertises elicitation
    /// support as `{"elicitation": {}}` in client capabilities.
    #[serde(default)]
    pub elicitation: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use crate::mcp_middleware::*;

    #[test]
    fn test_init_payload() {
        let init_payload = "{\"jsonrpc\":\"2.0\",\"method\":\"initialize\",\"id\":1,\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"openai-mcp\",\"version\":\"1.0.0\"}}}";

        let mpc_payload = McpInputPayload::try_parse(init_payload.as_bytes()).unwrap();

        println!("Mcp Payload: {:?}", mpc_payload);
    }

    #[test]
    fn initialize_picks_up_elicitation_capability() {
        let payload = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}},"clientInfo":{"name":"claude-code","version":"0.5.0"}}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::Initialize(c) => {
                assert!(c.capabilities.elicitation.is_some(),
                    "elicitation capability not picked up");
            }
            other => panic!("expected Initialize, got {:?}", other),
        }
    }

    #[test]
    fn initialize_keeps_client_info() {
        let payload = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"0.5.0"}}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::Initialize(c) => {
                let client_info = c.client_info.expect("clientInfo must be kept");
                assert_eq!(client_info.name.as_deref(), Some("claude-code"));
                assert_eq!(client_info.version.as_deref(), Some("0.5.0"));
            }
            other => panic!("expected Initialize, got {:?}", other),
        }
    }

    #[test]
    fn initialize_without_client_info_is_accepted() {
        let payload = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::Initialize(c) => assert!(c.client_info.is_none()),
            other => panic!("expected Initialize, got {:?}", other),
        }
    }

    #[test]
    fn initialize_without_elicitation_capability() {
        let payload = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::Initialize(c) => assert!(c.capabilities.elicitation.is_none()),
            other => panic!("expected Initialize, got {:?}", other),
        }
    }

    #[test]
    fn parse_jsonrpc_response_routes_to_server_response() {
        let payload = r#"{"jsonrpc":"2.0","id":-1,"result":{"action":"accept","content":{"password":"x"}}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        assert_eq!(parsed.id, RequestId::Int(-1));
        match parsed.data {
            McpInputData::ServerResponse { result_json, error_json } => {
                assert!(error_json.is_none());
                let result_json = result_json.expect("result_json must be present");
                assert!(result_json.contains("\"accept\""));
                assert!(result_json.contains("\"password\""));
            }
            other => panic!("expected ServerResponse, got {:?}", other),
        }
    }

    #[test]
    fn parse_jsonrpc_error_response_routes_to_server_response() {
        let payload = r#"{"jsonrpc":"2.0","id":-2,"error":{"code":-32000,"message":"user cancelled"}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::ServerResponse { result_json, error_json } => {
                assert!(result_json.is_none());
                assert!(error_json.is_some());
            }
            other => panic!("expected ServerResponse, got {:?}", other),
        }
    }

    #[test]
    fn string_request_id_is_preserved() {
        let payload = r#"{"jsonrpc":"2.0","method":"tools/list","id":"req-abc"}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        assert_eq!(parsed.id, RequestId::Str("req-abc".to_string()));
    }

    #[test]
    fn missing_request_id_is_null() {
        let payload = r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        assert!(parsed.id.is_null());
        match parsed.data {
            McpInputData::Notification { method } => {
                assert_eq!(method, "notifications/cancelled");
            }
            other => panic!("expected Notification, got {:?}", other),
        }
    }

    #[test]
    fn float_request_id_kept_raw() {
        let payload = r#"{"jsonrpc":"2.0","method":"ping","id":1.5}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        assert_eq!(parsed.id, RequestId::Raw("1.5".to_string()));
    }

    #[test]
    fn tool_call_without_arguments_defaults_to_empty_object() {
        let payload = r#"{"jsonrpc":"2.0","method":"tools/call","id":2,"params":{"name":"echo"}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::ExecuteToolCall(model) => {
                assert_eq!(model.name, "echo");
                assert!(model.arguments.is_object());
                assert_eq!(model.arguments.as_object().unwrap().len(), 0);
            }
            other => panic!("expected ExecuteToolCall, got {:?}", other),
        }
    }

    #[test]
    fn resource_templates_list_is_routed() {
        let payload = r#"{"jsonrpc":"2.0","method":"resources/templates/list","id":3}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        assert!(matches!(parsed.data, McpInputData::ResourceTemplatesList));
    }

    #[test]
    fn resource_unsubscribe_is_routed() {
        let payload = r#"{"jsonrpc":"2.0","method":"resources/unsubscribe","id":4,"params":{"uri":"res://a"}}"#;
        let parsed = McpInputPayload::try_parse(payload.as_bytes()).unwrap();
        match parsed.data {
            McpInputData::UnsubscribeResource(model) => assert_eq!(model.uri, "res://a"),
            other => panic!("expected UnsubscribeResource, got {:?}", other),
        }
    }
}
