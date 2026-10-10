use std::sync::Arc;

use json_schema::{my_json, *};
use serde::{Serialize, de::DeserializeOwned};

use crate::mcp_middleware::{
    ExecutedToolCall, McpMiddlewareError, McpToolCallAbstract, McpToolCallExWithInstruction,
    McpToolCallWithInstruction, ToolCallContext,
};
use my_http_server::async_trait;

pub struct ToolCallExecutor<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    pub fn_name: &'static str,
    pub description: &'static str,
    pub holder: Arc<dyn McpToolCallWithInstruction<InputData, OutputData> + Send + Sync + 'static>,
}

#[async_trait::async_trait]
impl<InputData, OutputData> McpToolCallAbstract for ToolCallExecutor<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
{
    fn get_fn_name(&self) -> &str {
        &self.fn_name
    }

    fn get_description(&self) -> &str {
        &self.description
    }

    async fn get_input_params(&self) -> my_json::json_writer::JsonObjectWriter {
        InputData::get_description(false, None, false).await
    }

    async fn get_output_params(&self) -> my_json::json_writer::JsonObjectWriter {
        OutputData::get_description(false, None, true).await
    }

    async fn execute(
        &self,
        input: &str,
        ctx: ToolCallContext,
    ) -> Result<ExecutedToolCall, String> {
        let parse_result: Result<InputData, serde_json::Error> = serde_json::from_str(input);

        let output = match parse_result {
            Ok(input) => {
                self.holder
                    .execute_tool_call_with_instruction(input)
                    .await?
            }
            Err(err) => {
                let msg = deserialization_error_message(
                    input,
                    &err,
                    self.get_input_params().await.build(),
                );
                let error = err.to_string();

                ctx.errors
                    .report(McpMiddlewareError::ToolInputDeserialization {
                        session_id: ctx.session_id.as_str(),
                        tool_name: self.fn_name,
                        arguments: input,
                        error: error.as_str(),
                    })
                    .await;

                return Err(msg);
            }
        };

        let structured_json = match serde_json::to_string(&output.data) {
            Ok(structured_json) => structured_json,
            Err(err) => {
                // Used to be an `unwrap()`, which killed the response task
                // and left the client with an empty stream. It is a bug in
                // the tool's own output type, so say so in-band instead.
                let error = err.to_string();

                ctx.errors
                    .report(McpMiddlewareError::ToolOutputSerialization {
                        session_id: ctx.session_id.as_str(),
                        tool_name: self.fn_name,
                        arguments: input,
                        error: error.as_str(),
                    })
                    .await;

                return Err(format!(
                    "Can not serialize the result of {}. Msg: {}",
                    self.fn_name, error
                ));
            }
        };

        Ok(ExecutedToolCall {
            structured_json,
            instruction: output.instruction,
        })
    }
}

/// What the client is told when its arguments do not match the tool's
/// `inputSchema`.
///
/// The schema is appended on purpose: a bare `missing field \`pattern\``
/// is a dead end for the model that invented `query` for that field —
/// it has no way of knowing what the field should have been without
/// re-reading `tools/list`. With the schema in the refusal it can
/// self-correct on the very next turn.
fn deserialization_error_message(
    input: &str,
    err: &serde_json::Error,
    input_schema: String,
) -> String {
    format!(
        "Can not deserialize input data {}. Msg: {:?}. Expected schema: {}",
        input, err, input_schema
    )
}

/// Context-aware executor — used by [`super::McpMiddleware::register_tool_call_with_context`].
pub struct ToolCallExecutorEx<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static,
{
    pub fn_name: &'static str,
    pub description: &'static str,
    pub holder:
        Arc<dyn McpToolCallExWithInstruction<InputData, OutputData> + Send + Sync + 'static>,
}

#[async_trait::async_trait]
impl<InputData, OutputData> McpToolCallAbstract for ToolCallExecutorEx<InputData, OutputData>
where
    InputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
    OutputData: JsonTypeDescription + Sized + Send + Sync + 'static + Serialize + DeserializeOwned,
{
    fn get_fn_name(&self) -> &str {
        &self.fn_name
    }

    fn get_description(&self) -> &str {
        &self.description
    }

    async fn get_input_params(&self) -> my_json::json_writer::JsonObjectWriter {
        InputData::get_description(false, None, false).await
    }

    async fn get_output_params(&self) -> my_json::json_writer::JsonObjectWriter {
        OutputData::get_description(false, None, true).await
    }

    async fn execute(
        &self,
        input: &str,
        ctx: ToolCallContext,
    ) -> Result<ExecutedToolCall, String> {
        let parse_result: Result<InputData, serde_json::Error> = serde_json::from_str(input);

        let output = match parse_result {
            Ok(input) => {
                self.holder
                    .execute_tool_call_with_instruction(input, &ctx)
                    .await?
            }
            Err(err) => {
                let msg = deserialization_error_message(
                    input,
                    &err,
                    self.get_input_params().await.build(),
                );
                let error = err.to_string();

                ctx.errors
                    .report(McpMiddlewareError::ToolInputDeserialization {
                        session_id: ctx.session_id.as_str(),
                        tool_name: self.fn_name,
                        arguments: input,
                        error: error.as_str(),
                    })
                    .await;

                return Err(msg);
            }
        };

        let structured_json = match serde_json::to_string(&output.data) {
            Ok(structured_json) => structured_json,
            Err(err) => {
                // Used to be an `unwrap()`, which killed the response task
                // and left the client with an empty stream. It is a bug in
                // the tool's own output type, so say so in-band instead.
                let error = err.to_string();

                ctx.errors
                    .report(McpMiddlewareError::ToolOutputSerialization {
                        session_id: ctx.session_id.as_str(),
                        tool_name: self.fn_name,
                        arguments: input,
                        error: error.as_str(),
                    })
                    .await;

                return Err(format!(
                    "Can not serialize the result of {}. Msg: {}",
                    self.fn_name, error
                ));
            }
        };

        Ok(ExecutedToolCall {
            structured_json,
            instruction: output.instruction,
        })
    }
}
