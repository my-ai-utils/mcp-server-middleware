use serde::{Deserialize, Serialize};

/// Represents a prompt argument in the MCP protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// Represents a prompt definition in the MCP protocol
#[derive(Debug, Clone)]
pub struct PromptDefinition {
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

impl PromptDefinition {
    pub fn new(name: String, description: String) -> Self {
        Self {
            name,
            description,
            arguments: Vec::new(),
        }
    }

    pub fn with_argument(mut self, name: String, description: String, required: bool) -> Self {
        self.arguments.push(PromptArgument {
            name,
            description,
            required,
        });
        self
    }

    pub fn add_argument(&mut self, name: String, description: String, required: bool) {
        self.arguments.push(PromptArgument {
            name,
            description,
            required,
        });
    }
}
