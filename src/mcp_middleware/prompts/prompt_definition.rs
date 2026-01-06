/// Trait that must be implemented by prompt services to provide metadata
pub trait PromptDefinition {
    const PROMPT_NAME: &'static str;
    const DESCRIPTION: &'static str;
}

/// Represents a prompt argument in the MCP protocol
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}
