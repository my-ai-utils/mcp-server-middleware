use super::*;
use std::ops::Bound;
use std::sync::Arc;

const PAGE_SIZE: usize = 100;

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
            return executor.read().await;
        }

        Err(format!("Resource with URI {} is not found", uri))
    }

    pub async fn get_list(
        &self,
        cursor: Option<&str>,
    ) -> (Vec<ResourceSchemaData>, Option<String>) {
        let lower = match cursor {
            Some(c) => Bound::Excluded(c.to_string()),
            None => Bound::Unbounded,
        };

        let mut iter = self.resources.range((lower, Bound::Unbounded));

        let mut result = Vec::with_capacity(PAGE_SIZE);
        let mut last_uri: Option<String> = None;

        for _ in 0..PAGE_SIZE {
            match iter.next() {
                Some((uri, resource)) => {
                    last_uri = Some(uri.clone());
                    result.push(ResourceSchemaData {
                        resource: resource.clone(),
                    });
                }
                None => break,
            }
        }

        let next_cursor = if iter.next().is_some() { last_uri } else { None };

        (result, next_cursor)
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
