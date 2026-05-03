use my_ai_agent::{json_schema::*, my_json};
use my_http_server::async_trait;

pub struct ToolCallOutput<T> {
    pub data: T,
    pub instruction: Option<String>,
}

impl<T> ToolCallOutput<T> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            instruction: None,
        }
    }

    pub fn with_instruction(data: T, instruction: impl Into<String>) -> Self {
        Self {
            data,
            instruction: Some(instruction.into()),
        }
    }
}

impl<T> From<T> for ToolCallOutput<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

pub struct ExecutedToolCall {
    pub structured_json: String,
    pub instruction: Option<String>,
}

#[async_trait::async_trait]
pub trait McpToolCall<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call(&self, model: InputData) -> Result<OutputData, String>;
}

#[async_trait::async_trait]
pub trait McpToolCallWithInstruction<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call_with_instruction(
        &self,
        model: InputData,
    ) -> Result<ToolCallOutput<OutputData>, String>;
}

#[async_trait::async_trait]
impl<InputData, OutputData, T> McpToolCallWithInstruction<InputData, OutputData> for T
where
    T: McpToolCall<InputData, OutputData> + Send + Sync + ?Sized,
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call_with_instruction(
        &self,
        model: InputData,
    ) -> Result<ToolCallOutput<OutputData>, String> {
        let data = <T as McpToolCall<InputData, OutputData>>::execute_tool_call(self, model).await?;
        Ok(ToolCallOutput::new(data))
    }
}

#[async_trait::async_trait]
pub trait McpToolCallAbstract {
    async fn execute(&self, input: &str) -> Result<ExecutedToolCall, String>;

    fn get_fn_name(&self) -> &str;
    fn get_description(&self) -> &str;
    async fn get_input_params(&self) -> my_json::json_writer::JsonObjectWriter;
    async fn get_output_params(&self) -> my_json::json_writer::JsonObjectWriter;
}
