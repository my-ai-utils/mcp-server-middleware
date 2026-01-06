use std::collections::HashMap;

use my_ai_agent::my_json::json_reader::JsonFirstLineIterator;
use serde::{Deserialize, Serialize};
#[derive(Debug)]
pub enum McpInputData {
    Initialize(InitializeMpcContract),
    ResourcesList(ResourcesListModel),
    ReadResource(ReadResourceModel),
    SubscribeResource(SubscribeResourceModel),
    NotificationsInitialize,
    ToolsList,
    PromptsList,
    ExecuteToolCall(ExecuteToolCallModel),
    GetPrompt(GetPromptModel),
    Ping,
    Other { method: String, data: String },
}

impl McpInputData {
    pub fn from_str(method: &str, params: String) -> Self {
        match method {
            "initialize" => {
                let params = serde_json::from_str(&params).unwrap();
                Self::Initialize(params)
            }
            "notifications/initialized" => Self::NotificationsInitialize,
            "resources/list" => {
                let model: Result<ResourcesListModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => {
                        return Self::ResourcesList(model);
                    }
                    Err(_) => {
                        // If params is empty or invalid, use default (no cursor)
                        return Self::ResourcesList(ResourcesListModel { cursor: None });
                    }
                }
            }
            "resources/read" => {
                let model: Result<ReadResourceModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => {
                        return Self::ReadResource(model);
                    }
                    Err(err) => {
                        panic!(
                            "Can not deserialize read resource data: {}. Err: {:?}",
                            params, err
                        );
                    }
                }
            }
            "resources/subscribe" => {
                let model: Result<SubscribeResourceModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => {
                        return Self::SubscribeResource(model);
                    }
                    Err(err) => {
                        panic!(
                            "Can not deserialize subscribe resource data: {}. Err: {:?}",
                            params, err
                        );
                    }
                }
            }
            "tools/list" => Self::ToolsList,
            "prompts/list" => Self::PromptsList,
            "prompts/get" => {
                let model: Result<GetPromptModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => {
                        return Self::GetPrompt(model);
                    }
                    Err(err) => {
                        panic!(
                            "Can not deserialize get prompt data: {}. Err: {:?}",
                            params, err
                        );
                    }
                }
            }
            "ping" => Self::Ping,
            "tools/call" => {
                let model: Result<ExecuteToolCallModel, serde_json::Error> =
                    serde_json::from_str(&params);
                match model {
                    Ok(model) => {
                        return Self::ExecuteToolCall(model);
                    }
                    Err(err) => {
                        panic!(
                            "Can not deserialize execute too call data: {}. Err: {:?}",
                            params, err
                        );
                    }
                }
            }
            _ => Self::Other {
                method: method.to_string(),
                data: params.to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteToolCallModel {
    pub name: String,
    pub arguments: serde_json::Value,
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

#[derive(Debug)]
pub struct McpInputPayload {
    pub _version: String,
    pub id: i64,
    pub data: McpInputData,
}

impl McpInputPayload {
    pub fn try_parse(src: &[u8]) -> Result<Self, String> {
        let json_iterator = JsonFirstLineIterator::new(src);

        let mut version: Option<String> = None;
        let mut method = None;
        let mut id: Option<i64> = None;
        let mut params = None;

        while let Some(item) = json_iterator.get_next() {
            let (name, value) = item.map_err(|err| format!("{:?}", err))?;

            let name = name.as_str().map_err(|err| format!("{:?}", err))?;

            let value = value.as_str();

            match name.as_str() {
                "jsonrpc" => {
                    version = value.map(|v| v.to_string());
                }
                "method" => {
                    method = value.map(|v| v.to_short_string());
                }
                "id" => {
                    if let Some(value) = value {
                        let Ok(id_value) = value.as_str().parse() else {
                            return Err(format!("Id is not number. {}", value.as_str()));
                        };

                        id = Some(id_value);
                    }
                }
                "params" => {
                    params = value.map(|v| v.to_string());
                }
                _ => {}
            }
        }

        let Some(version) = version else {
            return Err("Version is null".to_string());
        };

        let Some(method) = method else {
            return Err("Method is null".to_string());
        };

        let data = match params {
            Some(params) => McpInputData::from_str(method.as_str(), params),
            None => McpInputData::from_str(method.as_str(), String::new()),
        };

        Ok(Self {
            _version: version.to_string(),
            id: id.unwrap_or_default(),
            data,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeMpcContract {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
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
}
