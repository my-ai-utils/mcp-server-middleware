use super::PromptDefinition;
use std::collections::BTreeMap;

pub struct McpPrompts {
    prompts: BTreeMap<String, PromptDefinition>,
}

impl McpPrompts {
    pub fn new() -> Self {
        Self {
            prompts: BTreeMap::new(),
        }
    }

    pub fn has_prompts(&self) -> bool {
        !self.prompts.is_empty()
    }

    pub fn register(&mut self, prompt: PromptDefinition) {
        let name = prompt.name.clone();
        self.prompts.insert(name, prompt);
    }

    pub fn get_list(&self) -> Vec<&PromptDefinition> {
        self.prompts.values().collect()
    }

    pub fn get(&self, name: &str) -> Option<&PromptDefinition> {
        self.prompts.get(name)
    }
}

impl Default for McpPrompts {
    fn default() -> Self {
        Self::new()
    }
}
