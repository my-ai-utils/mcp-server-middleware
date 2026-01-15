use my_http_server::async_trait;

use crate::mcp_middleware::ResourceIcon;

pub struct ResourceReadResult {
    pub contents: Vec<ResourceContent>,
}

pub struct ResourceContent {
    pub uri: String,
    pub mime_type: String,
    /// Text content (direct string, not nested)
    pub text: Option<String>,
    /// Binary content (base64-encoded string)
    pub blob: Option<String>,
}

/// Trait that must be implemented by resource services to handle resource reading
#[async_trait::async_trait]
pub trait McpResourceService {
    async fn read_resource(&self) -> Result<ResourceReadResult, String>;
}

/// Abstract trait for resource services (similar to McpPromptAbstract for prompts)
#[async_trait::async_trait]
pub trait McpResourceAbstract {
    async fn read(&self) -> Result<ResourceReadResult, String>;

    fn get_resource_uri(&self) -> &str;
    fn get_resource_name(&self) -> &str;
    fn get_description(&self) -> &str;
    fn get_mime_type(&self) -> &str;

    /// Optional human-readable title for display purposes
    fn get_title(&self) -> Option<&str> {
        None
    }

    /// Optional size in bytes
    fn get_size(&self) -> Option<u64> {
        None
    }

    /// Optional icons for display in user interfaces
    fn get_icons(&self) -> Vec<ResourceIcon> {
        Vec::new()
    }
}
