use super::*;
use std::{collections::BTreeMap, sync::Arc};

pub struct PromptSchemaData {
    pub prompt: Arc<dyn McpPromptAbstract + Send + Sync + 'static>,
    pub argument_descriptions: Vec<PromptArgumentDescription>,
}

pub struct McpPrompts {
    prompts: BTreeMap<String, Arc<dyn McpPromptAbstract + Send + Sync + 'static>>,
}

impl McpPrompts {
    pub fn new() -> Self {
        Self {
            prompts: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, executor: Arc<dyn McpPromptAbstract + Send + Sync + 'static>) {
        let name = executor.get_prompt_name().to_string();
        self.prompts.insert(name, executor);
    }

    pub async fn execute(&self, prompt_name: &str, input: &str) -> Result<String, String> {
        if let Some(executor) = self.prompts.get(prompt_name) {
            return executor.execute(input).await;
        }

        Err(format!("Prompt with name {} is not found", prompt_name))
    }

    pub async fn get_list(&self) -> Vec<PromptSchemaData> {
        let mut result = Vec::with_capacity(self.prompts.len());

        for prompt in self.prompts.values() {
            let argument_descriptions = prompt.get_argument_descriptions().await;

            result.push(PromptSchemaData {
                prompt: prompt.clone(),
                argument_descriptions,
            });
        }

        result
    }
}

impl McpPrompts {
    pub fn get(&self, name: &str) -> Option<Arc<dyn McpPromptAbstract + Send + Sync + 'static>> {
        self.prompts.get(name).map(|p| p.clone())
    }

    pub fn has_prompts(&self) -> bool {
        !self.prompts.is_empty()
    }
}

impl Default for McpPrompts {
    fn default() -> Self {
        Self::new()
    }
}
