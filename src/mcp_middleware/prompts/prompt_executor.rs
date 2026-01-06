use std::{collections::HashMap, sync::Arc};

use crate::{
    PromptExecutionResult,
    mcp_middleware::{McpPromptAbstract, McpPromptService},
};
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

    async fn execute(
        &self,
        input: &HashMap<String, String>,
    ) -> Result<PromptExecutionResult, String> {
        self.holder.execute_prompt(input).await
    }
}
