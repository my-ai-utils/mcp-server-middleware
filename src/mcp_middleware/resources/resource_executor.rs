use std::sync::Arc;

use crate::mcp_middleware::{
    McpResourceAbstract, McpResourceService, ResourceIcon, ResourceReadResult,
};
use my_http_server::async_trait;

pub struct ResourceExecutor {
    pub resource_uri: &'static str,
    pub resource_name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
    pub title: Option<String>,
    pub size: Option<u64>,
    pub icons: Vec<ResourceIcon>,
    pub holder: Arc<dyn McpResourceService + Send + Sync + 'static>,
}

#[async_trait::async_trait]
impl McpResourceAbstract for ResourceExecutor {
    fn get_resource_uri(&self) -> &str {
        &self.resource_uri
    }

    fn get_resource_name(&self) -> &str {
        &self.resource_name
    }

    fn get_description(&self) -> &str {
        &self.description
    }

    fn get_mime_type(&self) -> &str {
        &self.mime_type
    }

    fn get_title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn get_size(&self) -> Option<u64> {
        self.size
    }

    fn get_icons(&self) -> Vec<ResourceIcon> {
        self.icons.clone()
    }

    async fn read(&self, uri: &str) -> Result<ResourceReadResult, String> {
        self.holder.read_resource(uri).await
    }
}
