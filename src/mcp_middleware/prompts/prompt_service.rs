use my_ai_agent::{json_schema::*, my_json};
use my_http_server::async_trait;

/// Trait that must be implemented by prompt services to handle prompt execution
#[async_trait::async_trait]
pub trait McpPromptService<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_prompt(&self, model: InputData) -> Result<OutputData, String>;
}

/// Abstract trait for prompt services (similar to McpServiceAbstract for tools)
#[async_trait::async_trait]
pub trait McpPromptAbstract {
    async fn execute(&self, input: &str) -> Result<String, String>;

    fn get_prompt_name(&self) -> &str;
    fn get_description(&self) -> &str;
    async fn get_input_params(&self) -> my_json::json_writer::JsonObjectWriter;
}
