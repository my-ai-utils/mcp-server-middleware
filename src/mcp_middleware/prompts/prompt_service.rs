use my_http_server::async_trait;
use std::collections::HashMap;

/// Trait that must be implemented by prompt services to handle prompt execution
/// The arguments are provided as a simple map of string key-value pairs
#[async_trait::async_trait]
pub trait McpPromptService {
    async fn execute_prompt(&self, arguments: HashMap<String, String>) -> Result<String, String>;
}

/// Abstract trait for prompt services (similar to McpServiceAbstract for tools)
#[async_trait::async_trait]
pub trait McpPromptAbstract {
    async fn execute(&self, input: &str) -> Result<String, String>;

    fn get_prompt_name(&self) -> &str;
    fn get_description(&self) -> &str;
    async fn get_argument_descriptions(&self) -> Vec<super::PromptArgumentDescription>;
}
