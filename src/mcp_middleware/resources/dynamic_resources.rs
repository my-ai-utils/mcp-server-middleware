use std::collections::BTreeMap;
use std::sync::Arc;

use super::{DynamicResourceExecutor, ResourceReadResult, ResourceSchemaData};

/// Runtime-mutable resource registry, mirroring [`McpResources`] for the
/// dynamic case. Lookup / list / has_resources match the static API so
/// `McpMiddleware` can fan out across both registries with minimal
/// branching at the call sites.
pub struct DynamicResources {
    items: BTreeMap<String, Arc<DynamicResourceExecutor>>,
}

impl DynamicResources {
    pub fn new() -> Self {
        Self {
            items: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, executor: Arc<DynamicResourceExecutor>) {
        let uri = executor.resource_uri.clone();
        self.items.insert(uri, executor);
    }

    pub fn remove(&mut self, uri: &str) -> bool {
        self.items.remove(uri).is_some()
    }

    pub fn has_resources(&self) -> bool {
        !self.items.is_empty()
    }

    pub fn contains(&self, uri: &str) -> bool {
        self.items.contains_key(uri)
    }

    pub async fn read(&self, uri: &str) -> Result<ResourceReadResult, String> {
        if let Some(executor) = self.items.get(uri) {
            return executor.holder.read_resource().await;
        }
        Err(format!("Dynamic resource {} not found", uri))
    }

    /// Snapshot of every dynamic resource as `ResourceSchemaData`. No
    /// pagination — dynamic registries are expected to stay in the
    /// "tens to low thousands" range for our use cases (per-message
    /// media in `telegram-ingest`). If a registry ever grows large
    /// enough that the unpaginated list becomes a problem, this is the
    /// place to bolt on cursor-based slicing.
    pub fn list(&self) -> Vec<ResourceSchemaData> {
        self.items
            .values()
            .map(|e| ResourceSchemaData {
                resource: e.clone(),
            })
            .collect()
    }
}

impl Default for DynamicResources {
    fn default() -> Self {
        Self::new()
    }
}
