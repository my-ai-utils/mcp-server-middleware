use my_ai_agent::{json_schema::*, my_json};
use my_http_server::async_trait;

use super::ToolCallContext;

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
    /// `ctx` is built by the middleware for every tool call. Existing
    /// `McpToolCall` impls ignore it; new `McpToolCallEx` impls use
    /// it for elicitation and other server→client interactions.
    async fn execute(
        &self,
        input: &str,
        ctx: ToolCallContext,
    ) -> Result<ExecutedToolCall, String>;

    fn get_fn_name(&self) -> &str;
    fn get_description(&self) -> &str;
    async fn get_input_params(&self) -> my_json::json_writer::JsonObjectWriter;
    async fn get_output_params(&self) -> my_json::json_writer::JsonObjectWriter;
}

/// Context-aware tool call. Implement this instead of [`McpToolCall`]
/// when you need to reach back to the client (e.g. to elicit input).
#[async_trait::async_trait]
pub trait McpToolCallEx<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call(
        &self,
        model: InputData,
        ctx: &ToolCallContext,
    ) -> Result<OutputData, String>;
}

/// Context-aware counterpart of [`McpToolCallWithInstruction`]:
/// implement it directly when a context-aware tool needs to attach an
/// `instruction` to its output. Every [`McpToolCallEx`] gets it for
/// free through the blanket impl below (instruction = None).
#[async_trait::async_trait]
pub trait McpToolCallExWithInstruction<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call_with_instruction(
        &self,
        model: InputData,
        ctx: &ToolCallContext,
    ) -> Result<ToolCallOutput<OutputData>, String>;
}

#[async_trait::async_trait]
impl<InputData, OutputData, T> McpToolCallExWithInstruction<InputData, OutputData> for T
where
    T: McpToolCallEx<InputData, OutputData> + Send + Sync + ?Sized,
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    async fn execute_tool_call_with_instruction(
        &self,
        model: InputData,
        ctx: &ToolCallContext,
    ) -> Result<ToolCallOutput<OutputData>, String> {
        let data =
            <T as McpToolCallEx<InputData, OutputData>>::execute_tool_call(self, model, ctx)
                .await?;
        Ok(ToolCallOutput::new(data))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::mcp_middleware::{McpElicitations, McpSessions, ToolCallContext};

    struct PlainExTool;

    #[async_trait::async_trait]
    impl McpToolCallEx<String, String> for PlainExTool {
        async fn execute_tool_call(
            &self,
            model: String,
            _ctx: &ToolCallContext,
        ) -> Result<String, String> {
            Ok(format!("plain:{}", model))
        }
    }

    struct InstructedExTool;

    #[async_trait::async_trait]
    impl McpToolCallExWithInstruction<String, String> for InstructedExTool {
        async fn execute_tool_call_with_instruction(
            &self,
            model: String,
            _ctx: &ToolCallContext,
        ) -> Result<ToolCallOutput<String>, String> {
            Ok(ToolCallOutput::with_instruction(model, "hint"))
        }
    }

    fn test_ctx() -> ToolCallContext {
        ToolCallContext {
            session_id: "test".to_string(),
            supports_elicitation: false,
            elicitations: Arc::new(McpElicitations::new()),
            sessions: Arc::new(McpSessions::new()),
        }
    }

    #[tokio::test]
    async fn blanket_impl_yields_no_instruction() {
        let ctx = test_ctx();
        let out = PlainExTool
            .execute_tool_call_with_instruction("x".to_string(), &ctx)
            .await
            .unwrap();
        assert_eq!(out.data, "plain:x");
        assert!(out.instruction.is_none());
    }

    #[tokio::test]
    async fn direct_impl_carries_instruction() {
        let ctx = test_ctx();
        let out = InstructedExTool
            .execute_tool_call_with_instruction("y".to_string(), &ctx)
            .await
            .unwrap();
        assert_eq!(out.data, "y");
        assert_eq!(out.instruction.as_deref(), Some("hint"));
    }
}
