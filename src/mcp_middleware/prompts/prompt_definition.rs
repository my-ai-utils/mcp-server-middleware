/// Trait that must be implemented by prompt services to provide metadata
pub trait PromptDefinition {
    const PROMPT_NAME: &'static str;
    const DESCRIPTION: &'static str;

    fn get_argument_descriptions() -> Vec<super::PromptArgumentDescription>;
}

/// Represents a prompt argument description in the MCP protocol
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptArgumentDescription {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}
