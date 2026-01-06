/// Trait that must be implemented by resource services to provide metadata
pub trait ResourceDefinition {
    const RESOURCE_URI: &'static str;
    const RESOURCE_NAME: &'static str;
    const DESCRIPTION: &'static str;
    const MIME_TYPE: &'static str;

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

/// Represents an icon for a resource
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResourceIcon {
    pub src: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(default)]
    pub sizes: Vec<String>,
}
