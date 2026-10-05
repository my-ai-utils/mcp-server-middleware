use std::collections::HashMap;

use my_http_server::async_trait;

use crate::mcp_middleware::ResourceReadResult;

/// Why a templated resource could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceTemplateReadError {
    /// The URI fits the template, but there is no resource behind it (an
    /// unknown topic, a deleted row). The client gets `-32002 Resource not
    /// found` with this message, which is a good place to list what does
    /// exist.
    NotFound(String),
    /// The resource exists but could not be read. The client gets
    /// `-32603 Internal error` with this message.
    Internal(String),
}

/// So a handler can `?` a `Result<_, String>` - such a failure is internal.
impl From<String> for ResourceTemplateReadError {
    fn from(err: String) -> Self {
        Self::Internal(err)
    }
}

/// Handles `resources/read` for every URI that matches a
/// [`ResourceTemplateDefinition`](crate::ResourceTemplateDefinition).
#[async_trait::async_trait]
pub trait McpResourceTemplateService {
    /// `uri` is the concrete URI the client asked for - put it into the
    /// returned contents as is. `variables` maps every variable of the
    /// template to its value, already percent-decoded.
    async fn read_resource(
        &self,
        uri: &str,
        variables: &HashMap<String, String>,
    ) -> Result<ResourceReadResult, ResourceTemplateReadError>;
}
