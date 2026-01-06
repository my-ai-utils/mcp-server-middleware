use std::{collections::HashMap, sync::Arc};

use crate::mcp_middleware::{McpPromptAbstract, McpPromptService};
use my_http_server::async_trait;

pub struct PromptExecutor {
    pub prompt_name: &'static str,
    pub description: &'static str,
    pub argument_descriptions: Vec<super::PromptArgumentDescription>,
    pub holder: Arc<dyn McpPromptService + Send + Sync + 'static>,
}

#[async_trait::async_trait]
impl McpPromptAbstract for PromptExecutor {
    fn get_prompt_name(&self) -> &str {
        &self.prompt_name
    }

    fn get_description(&self) -> &str {
        &self.description
    }

    async fn get_argument_descriptions(&self) -> Vec<super::PromptArgumentDescription> {
        self.argument_descriptions.clone()
    }

    async fn execute(&self, input: &str) -> Result<String, String> {
        // Parse the input JSON as a map of string key-value pairs
        let parse_result: Result<serde_json::Value, serde_json::Error> =
            serde_json::from_str(input);

        let arguments = match parse_result {
            Ok(value) => {
                // Convert JSON value to HashMap<String, String>
                let mut map = HashMap::new();
                if let Some(obj) = value.as_object() {
                    for (key, val) in obj.iter() {
                        // Convert value to string (handle different types)
                        let str_val = match val {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Null => String::new(),
                            _ => serde_json::to_string(val).unwrap_or_default(),
                        };
                        map.insert(key.clone(), str_val);
                    }
                }
                map
            }
            Err(err) => {
                let msg = format!("Can not deserialize input data {}. Msg: {:?}", input, err);
                println!("{}", msg);
                return Err(msg);
            }
        };

        self.holder.execute_prompt(arguments).await
    }
}
