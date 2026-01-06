use super::*;
use std::sync::Arc;

pub struct ResourceSchemaData {
    pub resource: Arc<dyn McpResourceAbstract + Send + Sync + 'static>,
}

pub struct McpResources {
    resources:
        std::collections::BTreeMap<String, Arc<dyn McpResourceAbstract + Send + Sync + 'static>>,
}

impl McpResources {
    pub fn new() -> Self {
        Self {
            resources: std::collections::BTreeMap::new(),
        }
    }

    pub fn add(&mut self, executor: Arc<dyn McpResourceAbstract + Send + Sync + 'static>) {
        let uri = executor.get_resource_uri().to_string();
        self.resources.insert(uri, executor);
    }

    pub async fn read(&self, uri: &str) -> Result<ResourceReadResult, String> {
        if let Some(executor) = self.resources.get(uri) {
            return executor.read(uri).await;
        }

        Err(format!("Resource with URI {} is not found", uri))
    }

    pub async fn get_list(&self) -> Vec<ResourceSchemaData> {
        let mut result = Vec::with_capacity(self.resources.len());

        for resource in self.resources.values() {
            result.push(ResourceSchemaData {
                resource: resource.clone(),
            });
        }

        result
    }
}

impl McpResources {
    pub fn get(&self, uri: &str) -> Option<Arc<dyn McpResourceAbstract + Send + Sync + 'static>> {
        self.resources.get(uri).map(|r| r.clone())
    }

    pub fn has_resources(&self) -> bool {
        !self.resources.is_empty()
    }
}

impl Default for McpResources {
    fn default() -> Self {
        Self::new()
    }
}
